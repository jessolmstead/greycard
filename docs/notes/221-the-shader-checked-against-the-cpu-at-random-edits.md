# 221. The shader checked against the CPU at random edits, and the GPU tests made to run in CI (2026-10-02)

Two gaps in the claim that every operation has a CPU reference and a
test: the viewport shader was checked against `finish_pixel` in ten
cases, most of them at the default edit, and no GPU test ran in CI at
all. A hosted runner has no adapter; each GPU test asked for one, said
SKIPPED and returned, and the step went green.

**The CI.** The Linux runner installs Mesa's Vulkan drivers, which
bring lavapipe, a software Vulkan implementation, and `cargo test`
runs there with `GREYCARD_REQUIRE_GPU` set and `WGPU_BACKEND=vulkan`.
Under the first, the four places a GPU test gives up without an
adapter (the render tests' `device()`, the op tests' `context()` and
the CA limits test's own request) fail instead of returning, so a
lavapipe that stops working cannot pass the tests by skipping them.
The second is there because wgpu ranks a GL adapter above a CPU Vulkan
one and a GL device made off any display is lost at creation; beside
lavapipe on a desktop the tests found the card's GL and failed. The
instances the tests and the op context make read wgpu's environment
for it. Mac and Windows are not held to either: Metal under a
virtualized runner and WARP are not promised, and the Linux run is the
one that counts.

**The test.** `the_shader_is_the_cpu_over_random_edits` in the render
tests draws 400 edits from a splitmix64 generator seeded per (seed,
index). A moved slider lands on its panel minimum a fifth of the time
and its maximum a fifth, since a uniform draw over a range almost never
reaches the corners, and the corners are where the first bug was: the
first version of the test, drawn uniformly, passed 1,600 edits on two
vendors with that bug put back. Each edit has every look slider, the
Light switch, both sources, all three output spaces, two or three
local adjustments of one to three shapes each (gradient, radial,
brush, learned raster, luminance, color) in random modes with looks of
their own, one of three look tables with a cross term, and one of two
guide planes, one coarse enough to make the reader interpolate. Grain
is in: both sides hash the same integers. The frame is 48 by 32 pixels
of near black, pure primaries, high chroma and highlights past white.
The CPU side is `finish_with` as the export calls it, through the
builders the export and the panel now share (`Local::of`,
`View::with_look`) and the export's own `reads_guide`, so a drift in
any of them shows. About nine seconds in debug on a desktop GPU and on
lavapipe alike, most of it the CPU's bracket. A failure prints the seed and the index and names
the parts whose removal brings the edit within tolerance;
`GREYCARD_PARITY_SEED`, `_EDITS`, `_ONLY` and `_SKIP` replay and
narrow a case.

**The bracket.** The two sides do not share arithmetic, so a pixel
whose answer is steep in its inputs cannot be held to a fixed
tolerance. Each pixel is bracketed by the CPU at rest and with each
channel nudged up and down by 1e-5 of the pixel's largest; the GPU
passes within 2.5/255 of that range. A steep pixel, one whose bracket
is wider than the tolerance, is still checked, so a NaN or a wrong
branch there shows; the share of steep pixels is capped at 10% an edit
and 1% over the run, so a broad break cannot hide as steepness. An
earlier form left steep pixels out instead, and skipped three thousand
comparisons to excuse one. The nudges are graded, 1e-5 and three
smaller steps, both signs, each channel, and a GPU value must sit
within the tolerance of one of the answers, not merely between the
least and the most of them, since a black would pass inside a wide
spread. The share cap is set from seeds 1 to 10, whose steepest edit
is 11%. Two cases in five thousand edits still sit outside the
bracket, both a mask weighing a few parts in a hundred million on one
side and nothing on the other at a black pixel, where a nudge that is
a share of the largest channel is zero; they are named in the `NUDGE`
comment. One draw is held back on purpose: a color curve's interior
points start at lightness 0.05 rather than the panel's 0.01, because
the curves' lightness axis is a cube root with unbounded slope at
zero, and a few parts in a billion of light the shader's fused
multiply leaves at a clipped channel read as a lightness near 1e-3
and a cast of whole levels. That sensitivity is the export's own, on
any device, and is on the roadmap.

**What it found.** Five divergences, each fixed on both sides in its
own commit:

- The shader's `tone` evaluated Narkowicz's fit for any input. With a
  global exposure, a mask's exposure, contrast and whites stacked, the
  fit's squares overflow to inf/inf, a NaN, which the clamp made
  black: a very bright pixel painted black in the viewport and white
  in the export. The CPU returns one at or past `DISPLAY_WHITE`; the
  shader now holds its input there, NaN included, which is the same
  curve (§145).
- Contrast is summed across the picture and its masks as departures
  from one, and a global 0.5 under a local at 0.5 sums to zero. The
  CPU's `powf(0, 0)` is one and turned black to mid grey; WGSL's `pow`
  at zero is undefined and went black. The sum is floored at 0.05 on
  both sides: two stacked flattenings cannot flatten past it, and a
  negative contrast is not a meaning on offer.
- The Oklab pass ran whenever the mixer's switch was on, which is the
  default, with every band at zero. A channel at exactly zero came out
  of it as a signed rounding residue, about 1e-7 of the largest
  channel from the CPU's `cbrt` and larger from the GPU's `pow`, often
  of opposite sign, and contrast under one with a gain after raised it
  to whole levels on one side only. The pass now runs only when the
  mixer, color, black and white or tint is not identity, and any
  channel it returns under 1e-6 of the largest is zero. Ordinary edits
  render the same to the channel; the stacked corners change by what
  the residue was.
- The shader read the brush rasters (R8Unorm) and the tone equalizer's
  guide plane (R16Float) through the sampler's bilinear filter. Vulkan
  holds a filtered read only to the format's own precision; NVIDIA and
  AMD give more, lavapipe gives about a third of an 8-bit level less,
  and two stacked looks over such a mask made up to 72 levels. On the
  two cards the test passed; on lavapipe 45 edits in 400 failed. Both
  are now read by texel and interpolated in f32 by the formula
  `Raster::at` and `Guide::at` use, so the viewport and the export
  read one mask and one guide on any conforming device.

- The Oklab pass rebuilt a and b from chroma and hue through cosine
  and sine, which Vulkan holds only to an absolute 2^-11. On a
  saturated primary the channel the pass leaves near zero took that
  error as its sign: the CPU landed just under zero, clamped, and a
  point curve lifted it to 0.388; Mesa landed just over, and the steep
  curve took it to 0. Both sides now scale the pixel's own a and b by
  the chroma's change and turn them by the mixer's hue shift alone,
  the same arithmetic with no trigonometry in the common case. NVIDIA
  agreed with the CPU by luck of sign.

The last two are the reason to run the GPU tests on a third
implementation at all: two vendors built to the same conventions agree with each
other in every place the specification leaves open, and a shader that
leans on a convention passes on both.

**What the contract looks like now.** Summed slider values under masks
go past the range any one stage was designed for, and that is where
the overflow, the zero contrast and the ill-conditioning all came
from; `WHITES_RANGE` clamps the whites alone. A tint exactly opposite
a pixel's hue has a real tie at 180°, where `Tint::applied` turns the
short way and rounding picks it. The look table is half-float on the
GPU and f32 in the export, unmeasured on its own. Not covered by the
test: the white-balance preview matrix, geometry (zoom, turns, crop,
perspective, the cubic reader), the export's unsharpened mask path,
the Background, Sky and Object shapes, the mask and clipping overlays,
the display table and soft proof, and the encoded-picture path.
