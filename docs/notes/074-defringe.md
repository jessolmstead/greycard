# 74. Defringe (2026-09-18)

§13 left axial chromatic aberration and purple fringing out of the
lateral correction: a radial shift of red and blue cannot undo a color
the lens smears along an edge at one focus distance. §61 found the same
thing from the other end, the lighthouse frame's out-of-focus sky gaps
lavender where Lightroom's are neutral, and put it on the list. This is
that tool.

**The port.** `develop::defringe` is RawTherapee's `PF_correct_RT` in
`rtengine/PF_correct_RT.cc`, Emil Martinec 2008-2010, optimized by Ingo
Weyrich in 2013 and 2018; GPL-3 with the attribution in the file header.
Blur a and b with a Gaussian of the radius; take each pixel's squared
distance from that local mean as its chroma deviation; a pixel whose
deviation is more than `5 (threshold / 33)²` times the frame's mean
deviation is fringing, and its a and b are replaced by the average of a
window of `ceil(2 radius) + 1` either way, each neighbor weighted by
`1 / (deviation + mean)` so the neighbors that are not themselves
fringing carry it. Lightness is never touched. RawTherapee's defaults,
radius 2 and threshold 13, are ours.

**Oklab, not Lab.** The reference works in CIELAB, which the engine does
not have; the mixer, the grading and the color curves read a hue in
Oklab, so the defringe does too, on the linear Rec.2020 working space.
The scale of a and b does not matter to any of it: every step is a ratio
of chroma deviations to their own mean, and the weights are one over a
deviation plus that mean, so a uniform factor cancels out of the test
and out of the weighted average alike. The hue does matter, and this is
worth stating rather than waving away. Measured on the two spaces: the
factor from CIELAB to Oklab is flat in lightness, about 1.4 from a
linear 0.02 to 4.0, but for the same perturbation of a color the ratio
of the spaces' deviations varies by about 2.9 around the hue circle,
blue and violet ranking higher than yellow and green. So a threshold of
13 here does not select the pixels a threshold of 13 selects in
RawTherapee: it leans toward the blues and violets. For purple fringing
that is the direction to lean, and 13 is still the sensible default, but
the two are not the same selection and a number carried across from
RawTherapee's forums will not land in the same place. The move brought Oklab's
matrices out of `greycard-ui/src/finish.rs` into `color::Oklab`, where
the engine's own working space defines them; the UI re-exports them, so
the mixer's numbers and the ones handed to the shader are as they were.

**Not darktable's.** `src/iop/defringe.c` is the same algorithm from the
same source, with three changes: it samples a Fibonacci lattice of 13 to
144 points instead of the whole window, it offers a local or a static
threshold beside the global one, and it grows the fringe region by a
pixel before averaging. The lattice is a speed trade the engine does not
need — the full window on a 45 MP frame is 0.30 s under rayon — and a
sparse average of a noisy chroma leaves mottling the full one does not.
The local threshold is the interesting part of the three and would be
worth having later on a frame whose fringing is all in one corner; the
module is deprecated upstream in favor of darktable's own chromatic
aberration correction, which is a lateral one and is what §13 already
is. So: RawTherapee's, whole window, global threshold.

**Where it runs.** After the lens correction, and on the worker's
path before the sharpen. Before the lens is wrong: the correction resamples, and a
cubic resample of a neutralized edge pulls the neighboring color back
across it. In the worker it sits in the base, right after
`correct_lens`, so it is cached with the base and a sharpen slider does
not redo it; in the CLI it is the tail of `correct_lens`. Worth saying,
since it came up while placing it: the sharpen is not in the same place
on the two paths. The CLI sharpens inside `develop`'s `finish`, before
the lens correction; the worker develops with the sharpen off, corrects
the lens, and sharpens the copy afterwards. With a profile that moves
pixels and the sharpen on, the CLI and the editor do not make the same
file. That is older than this change and is left alone here, but it is a
bug and belongs on the list.

**The edit.** Three fields on `Lens`, `defringe` off, `defringe_radius`
2.0 and `defringe_threshold` 13.0, and `Lens::defringe()` which answers
for them through the section's switch as everything else in that section
does. No schema version bump: §48 bumped because `noise.enabled` changed
meaning, and defaulted fields under `serde(default)` need none — a test
reads a lens block written before today and finds the defringe off with
the defaults in place. `Lens::is_identity` still answers only for what
moves or shades a pixel, since it is what decides whether the resample
runs; the defringe is asked for separately. Toggle and two sliders at
the foot of the LENS section, and `--defringe`, `--defringe-radius`,
`--defringe-threshold` on the CLI's develop.

**Checked.** Five tests on a synthetic frame with a neutral step edge
carrying a three-pixel purple fringe on its bright side and a flat
purple patch in a corner: the fringe drops to under a tenth of its
chroma, the patch keeps its a and b to a thousandth, the neutral sides
stay under 1e-4, lightness moves by less than 2e-4 anywhere, a frame of
one color and a frame smaller than the window come back untouched, and
a higher threshold touches less of the frame than a lower one.

On the 45 MP lighthouse frame (`4Z4A3525.CR3`, RF 50mm at f/1.2), at
the defaults, 0.30 s and 27.3 percent of the frame replaced, mean Oklab
chroma over those pixels 0.0130 to 0.0106. A quarter of the frame is a
lot to call fringing, and the reason is where the pass sits: on
scene-linear data that nothing has denoised, the mean deviation the
threshold is a multiple of is set by the noise, not by the edges, so
the bar is low and every noisy pixel clears it. RawTherapee's runs late,
on display-referred Lab after its denoise, where the mean is set by real
chroma edges. The pixels that clear the bar here mostly have no fringe
to lose and the weighted average hands them back what they had, which is
why the picture holds; but the number is not comparable with
RawTherapee's and should not be read as one. Two crops, measured at the
+1.9 stops of §61's edit:

| crop (200x200)          | off    | radius 2 | radius 4, thresh 8 |
|-------------------------|-------:|---------:|-------------------:|
| water sparkles @1730,4510 | 0.0236 | 0.0180   | 0.0163             |
| bokeh disc @1540,600      | 0.0219 | 0.0205   | 0.0194             |

The sparkles are the win and it is visible at 1:1: the magenta rims on
the specular points in the water go, the points stay bright. The bokeh
discs are not: their mean a and b do not move at all (-0.0115, -0.0121
before and after), because the lavender there is not a fringe but a cast
over a fifty-pixel disc, and a local mean taken over two pixels, or four,
says that disc is the neighborhood. §61's discs want a tool that reads
a whole out-of-focus region, which this is not. That is the honest
answer to the item and is now on the list as its own thing.

On the 24 MP orchids (`5M0A5391.CR3`, §13's 3.68 px at the corners,
which §13 already read as axial rather than lateral), 0.28 s. The corner
fringe, a 40x40 box at 5715,55 where an out-of-focus stem crosses a blown
window, is chroma 0.0625 at a +0.0115 b -0.0604, a clear violet. The
defaults take it to 0.0616, radius 5 at threshold 4 to 0.0567, and
threshold 0 with everything replaced only to 0.0565. The band is fifteen
pixels wide, so at any radius the window it is averaged over is mostly
itself. The same run leaves the saturated yellow flowers alone: a 300x300
crop of them measures 0.1097 before and 0.1105 after, and the two crops
are indistinguishable.

The CLI and the editor agree. §18's recipe, with the tone curve and the
sharpen off in a sidecar beside a link to the raw so the two finishes are
the same transform: the editor's full-size PNG export and the CLI's
`--preview` of the same file, both with `--defringe`, differ by 0.0009 of
255 on average with no pixel more than one step apart. The control, the
same export against the CLI's undefringed preview, differs by 0.27 of 255
with a peak of 124, so the defringe really is in both.

What was tried and did not pay: a larger radius on the broad fringes,
above; and darktable's Fibonacci lattice, which is only worth its
approximation when the window is not parallel, and this one is (rows in
parallel, a window per fringe pixel). What it costs beside the time: four
float planes of the frame while it runs, about 720 MB on 45 MP (this
first said five and 900 MB, a miscount found in §121, where the count
does become five), which is under the learned denoiser's and over
everything else in the develop.
