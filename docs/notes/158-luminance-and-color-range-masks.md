# 158. Luminance and color range masks (2026-09-23)

Roadmap v0.6.0's two mask lines, built in wave A by an opus author and
read twice by an opus reviewer.

Two new mask shapes on the Masks tab, the v0.6.0 roadmap line: a
window on the picture's lightness and a window on its hue and chroma,
each a `Shape` like a gradient or a brush, so they join, subtract,
intersect, invert and switch off with the machinery §27 and §81 built.

**The schema.** `Shape::Luminance { low, high, low_feather,
high_feather }` (serde tag `luminance`) and `Shape::Color { hue,
width, hue_feather, chroma, chroma_feather }` (tag `color`). New
variants of the tagged enum, no field of any old shape touched, so
every sidecar written before loads as it did and `VERSION` stands
(§16's rule). A round-trip test and a hand-written sidecar test hold
the tags.

**A shape from a later build.** A sidecar holding one of these opened
in a build from before them was refused whole ("unknown variant
'color' … starting from the default edit"), every other adjustment
with it. That cannot be mended for the builds already out, but the
next shape need not do it again: `Shape` now has a `#[serde(other)]`
unit variant, `Unknown`, which a kind this build does not know loads
as, whatever fields it carries. It counts as nothing: `Mask::live`
leaves it out, so it is not even joined (an unknown shape set to
Intersect does not empty the mask), and a mask of nothing but unknown
shapes is empty, not everywhere. `Mask`'s components are written
through a filter that drops it, so the next save removes it, and the
browser logs a warning naming the file and the count when it loads
one. Tested with a made-up kind with fields of its own between two
known shapes.

**What they read.** A range shape has no place of its own; it is
handed a `mask::Sample` at each pixel, which `finish::sample` makes and
the shader makes the same way: a pixel of the picture the finish is
handed, times the global exposure, to Oklab. That pixel has been
through the whole develop (white balance, camera profile, denoise,
lens corrections, the Detail section's texture, clarity and dehaze,
capture sharpen) and the geometry, and in an export made smaller the
resize; not the export's output sharpen, since `export::render` now
hands the finish the resized picture from before it as well
(`finish_with`'s `sampled`), so an export at any size samples the
stage the viewport does. None of the look: not the tone controls, the
curves, the mixer, the color section, the tint, a look table, the
vignette or any local. So a mask does not move under its own edit (a
luminance window on a sky with a stop and a half down on it would
otherwise darken the sky out of its own window and chase it), and no
range mask reads another's adjustments.

The Detail section is in the sample. Sampling the base from before it
would want that picture on the GPU as a second full-size texture,
since the viewport's texture is the developed picture after Detail and
capture sharpen, so it stays where it is and is said where it is read
(`finish::sample`'s doc, the guide). Measured by the reviewer on
`3G0A4650.CR3` with the sky window on the new scale (70 to 100, fading
6): dehaze at 0.6 moved 6.3% of the pixels (the window's coverage
36.7% to 30.5%). On the first version's window and scale clarity at
0.8 moved 0.6%, and contrast and highlights nothing, as they cannot:
they are not in the sample.

The global exposure is the one part of the look in the sample, on
purpose: it is a single gain over the whole frame, so it cannot make a
mask chase itself, and without it the numbers would mean a different
lightness on a frame shot two stops under and brought up than on one
exposed right. In the viewport the white balance preview's matrix is
in the sample too, as it is in everything the viewport shows before
the exact develop lands. Tested: a mask's own exposure pushed from one
stop to three leaves it on the same pixels, and three stops of global
exposure down takes a bright grey out of a window it was in.

**Luminance is Oklab L of the sample, not clipped, on a linear
scale.** Oklab L is a lightness, near CIE L*: the cube root puts equal
slider steps at equal visual steps, where a luminance Y (or stops of
it) crowds the highlights into the top few percent. It is the Oklab the
mixer, the tint and the vibrance already work in, so there is one
lightness in the program. The sample leaves out the 0.8 stops a raw is
shown brighter by (`Source::baseline`): 100 is the sensor's white at a
global exposure of 0, mid grey 57. The first version took the baseline
in and clipped L at 1, and the reviewer measured what that cost: L
reached 1 at a scene value of 0.574, so on `3G0A4650.CR3` 20.5% of the
frame sat at the top of the scale, shown between 0.94 and 1.0 grey,
the sliders could not tell a clipped highlight from a bright one, and
raising the exposure pushed more of the frame into that band. Now
nothing is clipped, and an edge at an end of the scale is open: a High
of 100 takes everything above it (a highlight the exposure has pushed
past the sensor's white included), a Low of 0 everything below. High is
never under Low: the sliders push each other, and `of_sample` reads a
High under Low as High at Low, so a sidecar written by hand cannot make
the empty mask or the bump between two fades the reviewer found. The
panel shows L from 0 to 100.

**The window shape.** Every edge names where the mask is at full
strength, and its fade is how far past that edge it takes to fall to
nothing, outward, by Hermite's step: the luminance window is one from
Low to High, falling over Low fade below and High fade above; the
color window is one within Width/2 degrees of Hue, falling over Hue
fade further round (the distance wrapping across 0/360), times one
from the Chroma floor up, falling over Chroma fade below it. A fade of
nothing is a hard edge, the edge itself in. Outward so the numbers a
person sets are the part that is surely in. A pixel with no chroma has
no hue and is in no color window: before, Rust's `atan2(0, 0)` gave it
hue 0, so with Chroma and its fade at 0 every true grey was red, and
with a fade longer than the floor a grey scored 0.896 in a window at
0. The chroma fade now runs down to no chroma at most (the sliders and
the code both clamp it to the floor). The color window reads its hue
and chroma from the 5x5 mean of Oklab a and b about the pixel, the
mean the mixer already reads a hue from (`local_ab`), so a noisy shadow
does not speckle in and out of it; its lightness is the pixel's own, so
a luminance edge is as sharp as the picture's.

**The dropper and the Skin preset.** Adding a Color shape puts a
dropper ("Range") in hand; a click samples the picture as the mask
does (the viewport's 5x5 box, the panel's white, the global exposure
through `Light::effective`, as the other droppers read it) and centers
the hue on it, saying so when the click is under the chroma floor. Pick
does the same later. Skin centers on 55 degrees, the vibrance
protection's skin hue (§60), but is wider than §60's shape. On
`066A3439.CR3`, a pale portrait, the face's hue runs from 10 to 59
degrees (forehead 16.7 to 59.4, nose 10.5 to 44.3, chin 12.6 to 55.1),
at chromas down to 0.017 to 0.019 on the new scale (the first
percentile 0.019 to 0.021). A window shaped as §60's
protection, full 15 degrees either side of 55 and gone by 45, gives the
skin 0.63 to 0.95 and the lips 0.36: a face in patches under a push, the
nose at 0.63. The preset is full from 25 to 85 degrees and gone by 0 or
110, full from a chroma of 0.02 and gone under 0.01; the nose goes to
0.93, and the cream cardigan beside it (chroma 0.0018 at its first
percentile to 0.0049 at its median), the
window frame's green (111 degrees) and the jeans (265) stay out. Her
strawberry blonde hair comes with it, as it would in any hue
selection: it is her skin's hue.

**The GPU.** `ShapeGpu` kinds 4 and 5 carry the windows; a `range`
vec4 at the end of the uniform says what the view's masks read (0
nothing, 1 the lightness, 2 the color too), the sample's exposure (the
global one, without the baseline, which `p.exposure` has in it) and,
for measuring, whether to draw the shown mask's weight alone as grey
(`View::mask_alone`, used by no UI). The read flag is set from every
local in the view, switched on or not: the overlay can show a
switched-off adjustment's mask, and the shader makes its weight
whatever its switch, so with the flag taken from the switched-on ones
alone (the first version) such a mask was drawn from an empty sample:
a window from 0 painted the whole frame, a color window nothing, and
which one depended on whether another adjustment happened to read the
picture. Checked on the frame with the reviewer's repro (the sky
adjustment switched off, its window 0 to 30): the overlay now paints
the land and not the sky.

The 5x5 mean is made only for a live color window, on both paths: on
the CPU it is the mixer's own array when the mixer reads the same
picture, and in the shader it is made once, at no exposure, and the
mixer's pass brings that one to its own exposure rather than making a
second (a test renders the mixer's picture with and without a color
window whose look does nothing and gets the same bytes). WGSL's
`smoothstep` is undefined when its edges meet, so the shader carries
`step_up`, the Rust `smoothstep` with its hard step.

**Checked against the CPU.** `the_shaders_range_masks_are_the_cpus`
renders five masks (a luminance window, the skin preset, a color window
at a blue, the skin intersected with a linear gradient, a luminance
window from 30 intersected with a gradient) over a synthetic field of
every hue across and every lightness down, reads them back and
compares with `Local::weight_sampled` on the same half-float pixels:
max 0.0022, mean 0.0003, the eight-bit read back's half level; every
mask covers part of the field. `a_switched_off_range_mask_shows_what_it_would_take`
does the same for a switched-off luminance window and color window
with nothing switched on. On a real frame (`4Z4A2978.CR3`, 5464x8192,
44.8 MP, developed at the defaults; the ignored
`a_real_frames_range_masks`, which now also requires every mask to
take in part of the frame, both intersections included: 14.5% and
26.1% over half): max 0.0109, mean 0.0003, with 19 pixels more than a
level out, all in the luminance window's High fade of 0.05, which
multiplies whatever small difference there is in what the two sides
read for a pixel by a slope of 30 per unit of L. The 1024 square from
the frame's middle, cut out as a texture of its own, agrees to 0.0022
max; the cause of the difference on the whole frame has not been
pinned down beyond that. Tolerance: 1.5/255 on the field and the crop,
4/255 over the whole frame.

**Cost.** On the CPU over that frame, 32 threads, the machine shared
with other builds (load 3 to 11): the finish of the whole frame, least
of five, 601 ms with no local, 715 ms with a luminance window and 742
ms with a color window. The mean alone measures 109 to 157 ms, and a
luminance window no longer pays it (the reviewer measured 117 ms of it
per 45 MP export before). An export with no range shape pays nothing
for them.

**Checked on the pictures.** With `--show-mask`: a luminance window
from 70 (fading 6) to 100 on `3G0A4650.CR3`, a cliff under an overcast
sky, paints the sky and none of the land or the sea; a darker bank of
cloud at the left edge stays out. A color window at 195 degrees on
`5M0A2279.CR3` paints the teal fur wall and the tracksuit and leaves
the pink neon, the blue glow round it and her face. The skin preset on
`066A3439.CR3` paints the face whole and the hair. Exported at 1600
pixels with the adjustment's exposure pushed and at nothing, mean grey
of patches:

- the sky a stop and a half down: its top third 0.936 to 0.747, the
  land (bottom 40%) 0.265 to 0.265, the sea 0.748 to 0.748;
- the skin preset a stop up, on the reviewer's face patches: forehead
  0.550 to 0.741, left cheek 0.530 to 0.724, right cheek 0.560 to
  0.751, nose 0.575 to 0.754, chin 0.515 to 0.701; on two patches of
  the author's own, the lips (40x10+480+535) 0.312 to 0.407 (partly in, as they
  are pinker) and the hair beside the cheek (25x60+400+450) 0.509 to
  0.702; the jeans, the foliage and the cardigan unchanged to the
  third decimal;
- the teal two stops down: the wall 0.318 to 0.105, the tracksuit
  0.456 to 0.297, her face and the neon sign unchanged.

The first version of this section measured the skin on two patches that
were brow and eye, and hair across the parting; these are the skin.

**Viewport and export differ in the color window's neighborhood.** The
5x5 mean is taken on the picture the finish is handed: the viewport's
texture is the developed picture at full size, an export made smaller
takes it on the resized picture, where five pixels cover more of the
scene. In the dark fur beside the hair on `5M0A2279.CR3`, the teal
wall, the full-size mask is speckled where the 1600-pixel export's is
smooth. The mixer
has the same difference, for the same reason; a mean at a fixed scale
would mend both and is not done here.

**Combination.** Nothing new: a range shape is a component, so "this
gradient, intersected with the bright part" is a Linear then a
Luminance set to Intersect, which the tests check on the CPU and the
GPU, and invert and the per-shape switch act on it as on any shape. A
range shape without a sample (`Mask::at`, a caller with no picture) is
nothing, as a raster shape without its raster is.

**The panel.** Luminance and Color buttons beside Subject and Object,
both under New and in a mask's Shapes; each makes its shape whole, with
no drag. The chosen range shape's sliders sit under Invert shape: Low,
Low fade, High, High fade; or Pick and Skin, then Hue (on the hue
circle's track), Width, Hue fade, Chroma, Chroma fade, the last five
with a wider label column (`EditSlider`'s `label-width` is now an
input) so "Chroma fade" is not cut. Choosing an adjustment whose shape
is a range opens the Shapes fold, since the sliders are the shape.
`docs/user-guide.md` has a section on them.

**Left.**
- Scopes weighted by the mask, the next roadmap line: the analysis
  pass draws through the same shader, so the weight is there
  (`weights[k]`); what it needs is a histogram mode that accumulates
  by it, and `Local::weight_sampled` is the CPU's side.
- The vibrance protection sits at 55 degrees plus or minus 15, the
  shape this item found too narrow for a pale face; whether §60 wants
  the same widening is open, and would change existing edits.
- A sample from before the Detail section, which needs a second
  full-size texture on the GPU.
- The mean at a fixed scale, for the viewport and a smaller export to
  agree on it (the mixer's as well).
- The pick reads the viewport's 5x5 mean of RGB and takes its Oklab
  hue, where the mask reads the mean of a and b: the same hue to a
  degree on anything but an edge. Its glue in `on_pick_pressed` has no
  test of its own (the test window has no renderer to sample); the
  sample it calls, the arming and the sliders do. A real click was not
  driven in the headless editor.
- No dropper for the luminance window's edges, and no picture of the
  window's colors beside the color sliders.
- The luminance window reads the pixel with no smoothing, so in noise
  its edge is as noisy as the picture.

**The review.** The numbers reproduced, and the reviewer agreed with
the global exposure in the sample. What it found: a switched-off
adjustment's mask drawn from an empty sample (above); the sample's
stage described wrongly, with dehaze measured moving a mask that the
docs said nothing moved; L clipped at 1 with the baseline in, which
put a fifth of a bright frame beyond the sliders' reach; Low above
High accepted silently; a gray pixel given hue 0; the pick reading
the raw exposure; the 5x5 mean paid by a luminance-only mask and
twice when the mixer was on; a sidecar with a new shape refused whole
by an older build; an explanation of the 84 worst pixels that named
the wrong cause and the wrong texture width; and the skin push
measured on patches that were brow and hair. Every one is above as
the design now is. The second pass reran the switched-off repro,
exported at two sizes to see the output sharpen no longer in the
sample, separated a clipped highlight from a bright one on the new
scale, pixel-diffed the Light section against master to confirm the
wider label column moved nothing else, and confirmed a sidecar saved
with an unknown shape dropped opens in master's binary.
