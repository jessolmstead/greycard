# §287. Detail in a mask (2026-10-10)

A mask's look has Light, Color, Curves, the mixer, Grading, a Tint and
a white balance (§241). It has no Detail: Texture, Clarity and Dehaze
are the whole picture's or nobody's. That is the first thing a
portrait asks for that the mask cannot give: Texture taken down on the
skin and left on the hair, Clarity on the eyes alone, a far ridge
dehazed under a sky left as shot. This section is the design; the
build follows it as its own section.

## Why the detail ops are different from the rest of a look

Everything in a mask's look today is a per-pixel function: the finish
(`finish_pixel_with`) takes the developed pixel, the global look and
each local's look weighted by its mask at that pixel, and makes the
output pixel from those alone. The detail ops are not per-pixel.
Texture and Clarity are bands of the log luminance, each the picture
less an edge-keeping base of it from a guided filter at a radius
(§103's local contrast), and Dehaze is a transmission from a model
fitted on a reduced copy of the whole picture. They run in the develop,
before the sharpen, on the whole picture in the source's own pixels.

But what each op *does* with its band is per-pixel. Texture puts
`k · (u - b)` stops on the pixel as a gain, faded toward the clip,
where `u - b` is the band at that pixel and `k` the slider's amount;
Clarity the same with its own band and a mid-tone weight on the base;
Dehaze moves the pixel toward or away from the airlight by the
transmission, `(px - A) / t + A`, with `t` from the model's haze at
the pixel and the amount. The expensive part, the bands and the haze,
does not depend on the amount. The amount enters at the end, per
pixel, and that is where a mask's weight can enter beside it.

## The rule

The develop's detail stage makes, beside the picture, the **detail
planes**: three values a source pixel, kept with the picture through
the sharpen and the geometry to the finish.

- `texture`: Texture's band at the pixel, `u - b` in stops, the log
  luminance less its guided base at the texture radius.
- `clarity`: Clarity's band, the low-passed log less its guided base
  at the clarity radius, already multiplied by the mid-tone weight of
  the base, in stops.
- `haze`: what the dehaze model would subtract from the transmission
  at full amount, so that `t(a) = 1 - a · S · haze` for an amount `a`
  (`S` the dehaze's `FULL_STRENGTH`), before the floor and the
  consistency bound the apply puts on `t`. The airlight `A` goes with
  the planes as one value for the picture.

The planes are made whenever any enabled mask has a detail amount
that is not zero, whether or not the global Detail is on; the global
Detail runs exactly as it does today, on the picture, and the planes
are the same bands it used. With no mask asking, no planes are made
and nothing changes.

The finish then applies, per pixel, after the locals' white balance
and before the exposure, as the develop applies the global's before
the sharpen:

    k_t = Σ_i w_i · texture_i · TEXTURE_GAIN
    k_c = Σ_i w_i · clarity_i · CLARITY_GAIN
    gain = exp2(clip · (k_t · texture + k_c · clarity))
    px *= gain
    a = Σ_i w_i · dehaze_i, clamped to [-1, 1]
    t = bounded(1 - a · S · haze)
    px = (px - A) / t + A

`w_i` is the mask's weight at the pixel, read as it is for the rest of
the look. `clip` is the same fade toward the develop's clip level the
global uses, from the pixel's brightest channel. `bounded` is the
apply's floor and the consistency bound (`t ≥ 1 - px_c / A_c` on each
channel), computed from the pixel the finish holds. Texture and
Clarity are in stops, so a mask at half weight gives half the stops,
which is what a feathered edge should do, and two masks over one
pixel add their stops as the global and a mask's would. A mask's
Dehaze adds its amount under its weight to what the global took out:
the transmissions compose multiplicatively, so clearing by the global's
`t_g` in the develop and then by the local's `t_l` at the finish is
clearing once by `t_g · t_l`.

A mask's Detail has the section's switch, as the global has: off, the
three sliders stay set and do nothing, so the comparison is a click.

**Why at the finish and not in the develop.** The develop is in the
source's pixels; the masks live in the frame (a crop's and a turn's
coordinates, with range shapes that read the picture the finish
holds). Weighting the detail in the develop would mean carrying every
mask back through the geometry and sampling the range shapes on a
picture the finish has not made yet. Carrying three planes forward
instead is one resample more, the same the picture gets, and the
weights are read exactly where every other part of the look reads
them. The order of the ops moves a little: a mask's Texture lands
after the sharpen, where the global's lands before it. The sharpen
reads its own planes from the picture before either, and on skin
taken down a stop of texture the difference is not visible; the build
measures it (below) and says what it found.

**Why the planes and not a second develop a mask.** A mask could have
its own picture, the develop run again with its amounts and blended
in by its weight. That is a develop a mask, and the local contrast's
filters are most of the detail stage's time; the planes are the same
filters once, and the rest is three multiplies a pixel a mask.

## What it costs

Three planes at the source's size: as f16, six bytes a pixel, 150 MB
at 24 MP and 270 MB at 45 MP on the CPU, and a `rgba16f` texture on
the GPU, 360 MB at 45 MP (the sharpen holds 900 MB of planes there
already, the local contrast up to 950 MB when it keeps its own). Made
only when a mask asks, so an edit without a Detail in any mask pays
nothing. When the global Detail is off and a mask asks, the develop
runs the local contrast's filters it would otherwise have skipped: a
few milliseconds on the GPU, a few hundred on the CPU at 45 MP, the
same cost the global Detail has when it is on. The dehaze's model is
fitted when a mask asks for Dehaze and the global has none, on the
reduced copy as the global's is.

## Format

`Look` gains `detail: Detail`, the same struct the global edit has
(switch, texture, clarity, dehaze), left out of the sidecar at its
default so every sidecar written so far is unchanged, and never read
on the global look, whose Detail is the edit's own. An older build
reads a look with `detail` in it and ignores the field (the look is
`#[serde(default)]` without `deny_unknown_fields`; the build confirms
this with a test as §283 did), so the picture it shows lacks the
mask's detail and it drops the field on its next save. The develop key
(§278) hashes the whole edit, so a mask's detail set changes the key
and the thumbnails follow; `DEVELOP_RECIPE` stays, since an edit
without the field makes the same picture as before. No `VERSION`
bump: a defaulted field.

## The panel

The Masks tab gains a DETAIL section after TINT, with the three
sliders and the switch, bound to the chosen mask's look as the other
sections are, through `show_edit` and the read-back. Its sliders are
look controls, so moving one hides the mask's overlay (§277). History
rows read "Mask: Texture", "Mask: Clarity", "Mask: Dehaze" and "Mask:
Detail on/off", as the Light rows do. The copy settings sheet carries
a mask whole, so nothing changes there; a preset with a mask carries
its detail with it. The scopes and the range masks read the picture
as they did: the weights are read before the look, and the scopes
weigh by the mask alone.

## Everywhere a picture is made

The export's finish (`render_framed`) resamples the planes into the
frame with the picture, by the same taps, and the CPU finish reads
them. The viewport's finish shader samples a planes texture at the
source positions it samples the picture at. The reduced develop
(§285) runs the detail ops on the CPU at its size and makes the planes
the same way, so a frame edited without being opened shows its masks'
detail in the strip; its check against the export covers it. The
droppers' `pick`, which reads one pixel, reads no planes and ignores a
mask's detail, as it ignores the vignette.

## Tests

- The CPU reference: a mask over the whole frame at weight one with
  Texture (then Clarity, then Dehaze) at an amount, the global Detail
  off, the sharpen off and the geometry the identity, equals the global
  Detail at the same amount to the planes' f16 rounding; at weight
  one half, half the stops; two masks at half each equal one at one.
- The GPU against the CPU: §221's random check gains a Detail in each
  local's look, drawn as the other sliders are, and holds to its
  limits on lavapipe.
- With no mask asking, the develop makes no planes, and the export is
  the same bytes as before.
- An old build's schema reads a look with `detail` as the look without
  it; a sidecar without the field reads as no detail.
- The reduced develop's check (§285) with a mask's Texture and Dehaze
  drawn among the edits, within its limits.
- The panel: a mask's Texture moved, the overlay hidden, the history
  row, the read-back through the sidecar's JSON and `show_edit`.
- Measured: the ordering difference against the sharpen on a skin
  texture of -0.5 and a sharpen at its default, in ΔE over the mask,
  reported in the build's section; and the develop's time with and
  without a mask asking, at 24 and 45 MP, on the CPU and the GPU.
