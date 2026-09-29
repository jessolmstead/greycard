# 19. The tone sliders (2026-09-06, late)

Asked whether the tone controls should be sliders first, a curve
editor first, or both, we chose sliders first: highlights,
shadows, whites and blacks under exposure and contrast, as in
Lightroom's basic panel and darktable's. A curve editor can come later
on top of the same edit.

**What they are.** Each is a physical quantity in the edit (§16) and
the same code on both paths (§18). After exposure and contrast, the
pixel's luminance (Rec.2020 weights, since that is the working space)
gives its position in stops about mid grey, `l`, and the three shifts
sum into one gain applied to all three channels alike, so a color
keeps its hue and its channel ratios, unlike a per-channel curve:

- highlights, in stops, weighted `smoothstep(0, 3, l)`: nothing at mid
  grey, full three stops above it;
- shadows, in stops, weighted `1 - smoothstep(-3, 0, l)`: nothing at
  mid grey, full three stops below;
- whites, in stops, weighted `smoothstep(2, 5, l)`: the top only,
  from two stops above grey (scene white is 2.47);
- blacks, a black point as a fraction of mid grey with scene white
  held: `(c - b) / (1 - b)` with `b = -blacks * 0.18`, so negative
  crushes that value to zero and positive lifts zero to a matte grey.
  A shift in stops is useless here (a stop at 0.005 is invisible); an
  offset is what a black point is.

Then the ACES shoulder as before. Sliders: ±1.5 stops for highlights
and shadows, ±1 for whites, ±30 percent for blacks. Mid grey is where
the shifts leave it; the black point moves it a little (a fifth of a
stop at the slider's end), which exposure recovers.

**Monotonic by construction.** Shifting by a function of `l` turns the
curve back on itself where `1 + d(shift)/dl < 0`. A smoothstep of
width `w` has slope at most `1.5 / w`, so highlights or shadows at
±1.5 stops over three stops reach 0.75, whites at ±1 over three
reaches 0.5, and where the highlights and whites ramps overlap the
sum stays under one. A test sweeps a grey from eight stops below mid
grey to six above at all sixteen corners of the four sliders and
asserts the output never falls. The ramps are as narrow as that
allows; a curve editor will need the same guard on what it draws.

**Checked on the GPU.** The viewport at 1:1 and the export's crop with
all four sliders off center (highlights −1, shadows +0.8, whites
+0.4, blacks −0.1, contrast 1.2, +0.3 EV, from a sidecar) differ by
0.16 of 255 RMSE and 0.03 of 255 on average, the display profile off
on both. The uniform grew by four floats and the vec4s after it
needed two pads again to land on 16 bytes; wgpu says so plainly
("bound with size 152 where the shader expects 160") before a frame.

**What these are not.** Lightroom's highlights and shadows are local:
a blurred luminance mask decides what counts as a highlight, so a
bright sky comes down without the bright edge of a face. These are
global, a tone curve in pieces, and will read as such on a picture
with both. A local version is a mask and a blur on the working image,
a develop op rather than a display one, and goes with local edits on
the roadmap (§15).

**Sliders and the wheel.** Also at our own request: the wheel moves
a slider that has focus (a click on it gives focus) or any slider
with Shift held, one step per notch, a touchpad's small deltas summed
to steps; otherwise the event passes through. The view is that the
wheel over a panel of sliders should scroll the panel unless the user
says which slider they mean.
