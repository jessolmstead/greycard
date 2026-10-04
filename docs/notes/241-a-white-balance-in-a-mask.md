# 241. A white balance in a mask (2026-10-04)

Mixed light: a room under tungsten with a window, a stage under two
colors of lamp, a church whose altar is lit warmer than its nave. The
global white balance can be right for one of them. A mask now carries
a white balance of its own, which says what the light is where the
mask is.

**Absolute, and off.** A mask's white balance names a temperature and
a Duv, "the light here is 3200 K", resolved through the picture's
camera profile exactly as the global's is. It does not follow the
global: move the global from 5500 K to 6500 K and the lamp under the
mask is still a 3200 K lamp, so the picture under it holds still. A
relative shift ("this much warmer than the rest") is the other way
to say it, and a fair one, but not this section's. The schema leaves
room for it: `Look::white_balance` is a `LocalWhite`, tagged by
`mode`, `Off` or `Absolute { temperature, tint }`, with `Off` the
default, left out of a sidecar while off, and the variant serde reads
any mode it does not know as, so a later `relative` written by a newer
build reads as off here rather than failing the sidecar. A defaulted
field, so no `VERSION` bump (§16). It is on `Look`, as the tint is
(§83), because a mask carries a look; but the global look never reads
it — the picture's own white is the edit's `white_balance`, in the
develop — so `Edit::look()` hands back `Off` and `set_look` ignores
it. A preset carries a mask's with the mask, in the Adjustments
section; there is no section of its own, since the global's is White
balance already.

**The arithmetic, once.** The develop applies the white as gains in
camera space before the camera matrix; a mask cannot reach in there,
since the develop makes one picture. But the change from one white to
another after the develop is exact for the gains and the matrix: back
through the develop's camera-to-working matrix, the ratio of the
gains, forward through the matrix the new white asks for,
`Mᵢ = target.matrix · diag(target.gains / base.gains) · base.matrix⁻¹`.
The viewport's white balance preview has made exactly this matrix
since the beginning, in the panel. It moved to
`greycard_core::color::white_shift` (arithmetic on gains and rows, no
edit schema in the engine), and `greycard_edit::WhiteShift` holds what
a local's is made from — the develop's resolved white and the profile
it went through — with `local(&LocalWhite)`. The panel's preview
calls the same function, so the preview and a mask cannot read one
white two ways. The headless export gets its `WhiteShift` from its own
develop: the worker's base now keeps the profile its white was
resolved through (the edit's DCP, else the file's own, chosen as the
engine chooses it), so `--export` builds the same matrices with no
window and no panel.

**Where in the pass.** First, on the developed picture, before the
exposure and everything else in the look, as the develop's own white
is under everything: the mask's light sliders, mixer and tint then act
on the picture at the white the mask names. After the masks' weights,
which read the picture as it came from the develop; so a luminance or
color range mask is never moved by its own white balance, or by
another mask's. Per pixel, `M = I + Σ wᵢ (Mᵢ − I)`, the blend every
other parameter has (§27): two masks at half each are one at full, and
a feathered edge walks between the two whites. In the viewport the
blend is over the preview's matrix rather than over the identity
(`W + Σ wᵢ (Mᵢ − W)`): a mask's white is absolute, so at full weight
it is `Mᵢ` whatever the global slider is doing while its develop is on
the way. In the export `W` is the identity and the two are one.

**The mixer's mean goes with it.** The mixer and the black and white
read a pixel's hue from the mean Oklab a and b about it (§63), made
once from the developed picture. Left alone, a wall a mask turns from
orange to grey would still be orange to the mixer, and the orange
band would act on a grey wall. So under a local white the mean is
taken with the pixel: rebuilt as a color at the pixel's own lightness,
put through the same matrix, and read again (`finish::rebalanced`, and
the shader's same lines). A flat patch, whose mean is its own color,
comes out exactly the white-balanced pixel's a and b. In the viewport
the mean is already of the picture at the preview's white (each tap
of `local_ab` goes through `W`), so there the pixel's lightness is
read at `W·t` and the mean goes through the locals' matrix relative
to the preview's, `(W + Σ wᵢ (Mᵢ − W)) · W⁻¹`, not through `W` a
second time; a first version did, which was right at rest and off by
up to 30 levels mid-drag in a test made for it. The tone equalizer's
guide plane is left as it is: it is a smoothed luminance, and a white
balance moves luminance little and smoothly.

**Held, and read.** A picture held on screen while the next frame
develops (§57) keeps its masks' whites: what they were made from is
kept from its own develop, since the frame, its profile and the
develop's white have moved on to the next file by then. A mask's
white is held to the panel's range where it is read, 2000 to 12000 K
and 0.05 Duv either way — past them the matrix after the develop
extrapolates the camera's calibrations into colors no light makes,
and a hand-edited 1e9 K would be one — while the global's is left as
it always was. And each field defaults, a temperature left out to
5500 K and a tint to none, so a hand-edited mask missing one costs
that field and not the whole sidecar.

**With a DCP.** A mask's white is the profile's matrices only: the
develop blends a DCP's hue/saturation map at the global white's
temperature (§122), and the map stays there under a mask, so with a
dual-illuminant DCP a mask at another light takes the right matrix and
the global's map.

**The panel.** On the Masks tab, with a mask chosen, a WHITE BALANCE
section under its name, before LIGHT: a switch on the title,
Temperature and Tint on the global's tracks and ranges, and Neutral;
no As shot and no Auto, which are the picture's. Switched on, it
starts at the global's white — As shot read as the kelvin and Duv the
camera's gains mean — so nothing jumps until a slider moves. The one exception is an As shot outside the mask's range, a camera reading under 2000 K or past 0.05 Duv: the mask starts at the range's edge, the most its slider could show, and the masked area moves that far when the switch goes on. A custom global cannot be out there, since its sliders, the dropper and Auto are held to the same range; widening the mask's range and its sliders together is the answer if a real frame ever wants it. The
Neutral dropper on the Masks tab sets the mask's and switches it on:
the white that makes the patch grey is the light there, the same
answer whatever the global says, since the mask's is absolute, and
under the mask at full weight the patch is then grey. The Develop
tab is as it was. History reads "Lamp: White balance on", "Lamp: White
balance 3200 K", "Lamp: Tint -0.002", "Lamp: White balance off". The
scopes weighed by a mask (§189) are drawn by the same shader and see
it; the curve and mixer droppers read the global look and no mask's,
as before, so there is nothing for them to see.

**Clipped highlights: measured, and left.** Highlight reconstruction
and the clip happen at the develop's gains, where a fully clipped
pixel is sent to neutral. A mask's matrix then takes that neutral to
its own white's color, which a global develop at that white would not
do. We measured it on `5M0A3976.CR3`, a church under tungsten with a
row of clipped candles: a mask over the whole frame at 3200 K over a
global 5500 K, against the global at 3200 K. At the default exposure
the two differ by 0.72 percent RMSE over the frame, against 5.9
between the two globals, and the candle strip (0.92) no more than the
rest (0.71): the display curve takes the clipped cores to white either
way. At −3 EV, where the cores are grey on the screen, they come out
plainly blue under the mask: 7.05 percent in the candle strip against
0.35 in the rest. We tried fading `Mᵢ` toward the identity by the
develop's camera-space value near `WhiteBase::clip` (the largest
balanced channel, through the base matrix's inverse, a smoothstep from
a fraction of the clip up to it). The cores go grey as the global's
are, but the ring around each, where only the red channel clipped
under the tungsten and the reconstruction filled it in, then keeps the
develop's 5500 K rendering and turns orange; the RMSE in the strip
barely moves (7.17, 6.97, 6.84 and 6.77 percent for a fade from 0.5,
0.75, 0.9 and 0.97 of the clip, the rest 0.33 to 0.30), and a narrow
fade draws a thin blue contour at the core's edge. Which pixels count
as clipped is the real question, and a single-channel clip is not
answered by a fade: that is a judgment, so it is left out. A mask
whose white differs strongly from the global over clipped highlights,
pulled well down, shows them tinted.

**Checked.** Viewport against export on `5M0A3976.CR3` at 6000x4000,
the view 1500x950 at 1:1 (`--hide-panels`), a feathered, tilted radial
mask at 3200 K over a global 5500 K: 0.067 percent RMSE and 0.011 MAE,
against 0.066 for the same edit with the mask's white off; the mask
moves that crop by 3.1 percent RMSE. A harder edit — the radial at
2700 K and Duv +0.006 with the mixer's orange turned and its aqua
raised, a linear gradient at 8000 K and Duv −0.01 a third of a stop up
crossing it, the global +0.7 EV — agrees to 0.068 percent and moves the
crop by 10.3. The random parity test now gives half of each edit's
masks a white balance over the panel's ranges, through a test profile
with a warm and a daylight calibration (`testing::white_shift`), drawn
after everything else so the earlier draws are every seed's as before;
seeds 1 to 10, 400 edits each, pass on the RTX 5070 Ti and on lavapipe
(llvmpipe). Tests: the shift lands where a develop at the target would,
and is the identity to itself; a mask's white at the global's own white
is the identity, and warmer light named makes a grey bluer; off reads
from no field and from a mode this build does not know; a field left
out defaults and the edit around it survives; a value that is not a
number is off, and 1e9 K with a tint of 0.5 is held to 12000 K and
0.05, the same matrix as the edge; the round trip, the old sidecar, the unmoved `VERSION`, a preset's
Adjustments carrying it and the global look carrying none; a mask's
white at full weight is the picture finished at that white, two halves
are one whole and a half is the matrix halfway; a local without one is
not touched at all; the mixer's mean taken with the pixel; a luminance
mask that its own white would push out of its window stays on; the
GPU against the CPU with a warm mask over everything (the mixer moved
under it) and a cool gradient across it, within a level, and again
with the view previewing 6500 K over a develop at 5500 K, against the
CPU handed the develop at 6500 K and the mask's matrix relative to it,
within two levels (30 with the mean through the preview twice); a held
picture keeping its masks' whites while the next frame's arrive; the panel's
switch starting at As shot's kelvin and Duv and at a custom global's,
the dropper setting the mask's and not the global's; the headless
export making the matrices from its own develop, the identity at the
develop's own white; and the history rows.

**Not.** No relative mode (the schema has room). No Lightroom import of
a local Temperature and Tint (`LocalTemperature`, `LocalTint`): the
importer reads global sections only (§52). No Auto on a mask, which
would want the auto white balance weighed by the mask. No DCP map at
the mask's light (above). The clip, above.

And found on the way, not fixed here: `--zoom 1` with `--screenshot`
opens fitted, since the camera's JPEG standing in for the develop
fits the view (`cull::placeholder_arrived`), and `--show-mask` is
dropped when the first file is opened a second time at startup; the
checks above ran on a build with both held (not committed).
