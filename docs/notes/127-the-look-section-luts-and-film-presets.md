# 127. The look section: LUTs and film presets (2026-09-20)

Looks: 3D LUTs, and film presets that need none (2026-09-20)

The second of §78's four, and the other end of the pipeline from
§122's camera profiles: a `look` section carrying a 3D LUT by name
and a strength, applied after the tone curve in the table's own
encoding, and a handful of film presets that are the parametric
controls and nothing else.

**The reader is in core, and it is one type.** `greycard-core/src/lut.rs`
reads `.cube` — `TITLE`, `DOMAIN_MIN`/`DOMAIN_MAX`, the older
`LUT_3D_INPUT_RANGE`, `LUT_3D_SIZE`, `LUT_1D_SIZE`, comments and
blank lines — and HaldCLUT PNG, into one `Lut3d`: a size, `size^3`
entries with red running fastest, a domain, an encoding and a set of
primaries. A HaldCLUT's level comes from the image's width, which is
the level cubed, and its cube is the level squared; the layout is the
same order a `.cube` writes, so the two land in the same array with no
special case downstream. A 1D `.cube` is a per-channel curve and is
spread over a cube of 64 nodes, or its own length where that is
shorter: tetrahedral interpolation is exact for a separable function
(within a cell only one edge of the tetrahedron moves each channel,
which the tests pin), so the expansion costs nothing between the nodes
either and the pipeline carries one kind of table rather than two.

Errors name what is wrong: "LUT_3D_SIZE 33 asks for 35937 rows, the
file has 35900", "line 12: \"blue\" is not a number", "the image is 26
wide, which is not a whole number cubed", "DOMAIN_MAX is not above
DOMAIN_MIN on the red axis", "the file declares both LUT_3D_SIZE and
LUT_1D_SIZE". 144 nodes an axis is the ceiling — 3.0 million entries,
36 MB of floats here and 24 MB as half floats on the GPU, and what a
HaldCLUT of level 12 comes to — and a bigger one is refused by name
rather than quietly resampled.

**Tetrahedral, not trilinear, and the reason is neutrals.** The six
tetrahedra of a cell all share its black-to-white diagonal, so a
neutral color is a straight blend of two neutral corners and comes out
neutral exactly, whatever the table does elsewhere. Trilinear mixes
all eight corners and cannot. The test that holds this down needs a
table that is neutral-preserving, not separable and not
permutation-symmetric — a symmetric one is neutral under trilinear too,
by symmetry — so it is red picking up `0.4 * (g*b - r*r)`, zero on the
neutral axis and not multilinear: tetrahedral keeps every grey to
within 1e-6, trilinear pushes them off by more than 1e-3. It is also
what every grading application resolves a `.cube` with, so a LUT looks
here as it looked where it was made.

**The encoding a file does not state.** The `.cube` format says
nothing about what its numbers are encoded in, and neither does a PNG.
Both are read as display-referred sRGB, in sRGB primaries, because
that is what every collection worth reading is. A `.cube` may say
otherwise in a comment, and two conventions of this build's own are
honored, case and punctuation aside:

    # encoding: srgb        (or linear, rec709, gamma 2.2)
    # primaries: srgb       (or rec2020, p3, adobergb, prophoto)

`GREYCARD_ENCODING` and `GREYCARD_SPACE` read as the same two, and
`space` and `colorspace` as the second. Nothing else is: a comment
this build does not know is a comment.

A declaration has to sit above the table. Not because reading one
below it would be hard, but because the listing stops at the first
row and the reader does not, and the two must not disagree about what
a file says it is — a panel note that reads "sRGB" for a table
rendered as linear is worse than one that says nothing. Same reason
the listing now stops at the first row and at nothing earlier: an
earlier cut stopped as soon as it had a size and a title, and missed
a declaration written under them.

Two more things a real `.cube` does that the first cut refused. A file
written by a grading application carries keywords this build has no
use for — `LUT_IN_VIDEO_RANGE` and its friends — and some editors put
a byte order mark on the front. Both came back as "is not a number",
which is a true statement about a file that is perfectly fine. A line
whose first word starts with a letter is now a keyword, known or not,
and an unknown one is passed over with a line in the debug log; the
mark is stripped.

And a header read reads a header: the first 64 KB of the file, not the
whole of it. A directory of three hundred film stocks at 64 nodes an
axis was about a gigabyte of parsing on the thread that draws the
panel, for a title and a size that sit in the first four lines. A file
whose header somehow runs past the chunk is read whole rather than
guessed at.

**The stage, and why it sits where it does.** `lut::Look` is the whole
thing a consumer wants, and it is §78's order: the working space's
linear color through a matrix into the table's primaries, clipped into
them, encoded, looked up tetrahedrally, decoded, brought back through
the inverse matrix, and blended with what came in by the strength. The
clip *is* the gamut map, and it is a hard one — an sRGB table has no
entry for a Rec.2020 green, and a soft map would be a second design
decision hidden inside a first. Nothing is lost by it in practice: by
the time the look runs, the tone curve has already clamped the picture
to 0..1 and the point curves have taken it through an encode and a
decode of the same range, so what the clip removes is the out-of-sRGB
saturation the table could not have described anyway.

Where it sits is after the tone curve, after the point curves and the
color curves, and before the output matrix — the last thing that is
still in the working space. That is the only place a table made for a
display can be read at all, and it is the one place the viewport and
the export both pass. The blend is in working linear, after the decode,
not in the table's encoding: half of a table that goes to black is half
the light, which is 0.73 of the encoded value, and that is the honest
reading of "half a look".

**Both paths, held together.** `finish_pixel_with` takes an
`Option<&lut::Look>` and runs `Look::at` in that slot;
`viewport.wgsl` gained a second 3D texture beside the monitor's
(binding 11) and does the same arithmetic — the same six-branch
tetrahedron, the same clip, the same encode and decode, the same
blend — with the two matrices, the domain and the encoding passed in
the uniform. The table is read by node rather than sampled, so it
needs no sampler and takes no filtering: the hardware's interpolation
is trilinear and that is the one thing this stage must not be.

The table is uploaded only when it is another table. A strength moved
on the slider changes four floats in the uniform, not a megabyte of
half floats, and the renderer remembers the `Arc` it holds to tell the
two apart. It lets go of both the moment the edit names no table: a
2x2x2 identity goes back on the binding, which must have something in
it, and a strength of zero is what the shader reads as nothing to do.

`Encoding::Gamma` is sign times the power of the magnitude, on both
paths. A plain `powf` of a negative number is NaN and a table is
allowed to hand one back; the clip on the way in means it cannot
happen going in, but the decode on the way out has no such guard.

**The §18 check.** A 45 MP CR3 cropped to 3000x2000, a hand-written
sidecar, `--no-display-profile`, the viewport at `--zoom 1` against
the export's matching crop, the crop averaged over the two rows the
shader's half-pixel sampling blends at an odd viewport height (1203):

- no look: **0.316% RMSE** (0.805 of 255)
- a 17-node `.cube` of my own making at strength 0.8: **0.270% RMSE**
  (0.689 of 255)

The look moves the picture by 4.5% RMSE, so the agreement is not the
agreement of two pictures that were never changed. It reads *better*
than the baseline because it desaturates and flattens, which shrinks
the residual the half-row resampling leaves behind.

What that residual is, is worth being exact about, because a
whole-frame RMSE cannot tell a shader that disagrees from a crop that
is half a row off. A half-row resample can only show where the picture
has detail: in a flat neighbourhood it has nothing to blend. Split the
frame on that — the pixels whose four neighbors are identical to the
last 8-bit level, which is the only "local gradient under half a
level" an 8-bit image can have — and the two sides separate cleanly:

| | flat (23% of the frame) | detail |
|---|---|---|
| with the look | RMSE 0.151/255, max **1/255** | RMSE 0.782/255, max 76/255 |
| no look | RMSE 0.187/255, max **1/255** | RMSE 0.816/255, max 66/255 |

Everywhere the picture is smooth, the shader and the CPU agree to
within one count of 255, with and without a look alike. Everything
above that sits on an edge, which is where the half-pixel sampling
lives and where it belongs. The shader is held to the CPU, as §18 has
it.

**The edit, and a name that was taken.** `Edit` already has a `Look`:
the light, the curves, the mixer, the color, the grading and the tint
that a mask can carry a version of (§83). This is the other thing the
word means, so the type is `look::LookLut`, the field is `look_lut`,
and serde renames it to `look` — which is what the roadmap, the
sidecar and the panel call it. Living with two meanings of one word
behind a rename beat renaming a type that half the crate uses.

The section is a choice — "none", or a file name resolved against
`$XDG_DATA_HOME/greycard/looks`, the profile directory's logic one
name along, with the same refusal of anything path-like — and a
strength, which defaults to 1: a look chosen is a look wanted, and the
slider is there to take it back, not to have to be found first. No
schema bump: the field has a default, so every sidecar written before
it reads unchanged and reads as no look, and `VERSION` stays 3. The
table is read in `LookLut::look()` and kept in a cache keyed by path,
size and mtime, four at most, so the viewport and the export are handed
the same table and a file that will not read warns once rather than on
every frame. The editor keeps the resolved look in its state and
re-resolves only when the section changes, since a stat of the file per
frame is a syscall for nothing.

A preset carries it — `Section::Look` — and carries it *by default*,
unlike the camera profile: a film preset is exactly a look and a
strength, and a look belongs to a picture's rendering, not to a body.

The history names the step, as every other section's does: "Look Test
Look" when the table changed, "Look strength 40%" when only the slider
moved, "Look none" when it went off. Adding a section to the edit and
forgetting `history::changes` is a silent bug — every look step read
"No change" until it was caught — and the only defence is that the
arm is part of adding the section, not a later thought.

`LookLut::look()` hands back a table whenever one is named, at
whatever strength, and `None` only when none is. The distinction earns
its keep at the other end: `None` is what tells the renderer it may
drop the texture and let go of the table, and a strength of zero is
not that — dragging the slider through zero and back would otherwise
throw away two megabytes and send them again.

**The panel.** A LOOK section after TINT, where the stage runs: an
Embedded-style row list with None first and then what the directory
holds, a Strength slider, and a line under it. The line is the chosen
table described — "3D .cube, 17 nodes", and what it says it was made
in when that is not the usual sRGB — or, for an empty directory, where
to put files. A table the edit names and the directory has not got is
still a row, still chosen, and carries a warning: the panel says what
the edit says, which is what a sidecar or a preset from another machine
needs. The directory is read again when the section is opened, so a
`.cube` dropped in while the editor is running is listed without
reopening the file, and that re-listing is also when the table held for
the last choice is let go of. Listing reads each file's header and
stops; only the chosen table is built.

A Reset beside the others, back to None at a full strength. The
directory is re-read when the section opens and the chosen table is
asked for again, which picks up a file replaced in place; nothing is
thrown away to do it, since `load` already reads a file afresh when
its size or its clock moved. An earlier cut cleared the cache on every
listing instead, which re-read the open picture's table and re-uploaded
it to the GPU each time the section was opened, and said whatever
warning it had to say twice.

Picking a look does not develop. It is the finish, and the picture the
engine hands over is the same one, so the choice redraws and saves and
that is all.

On the command line there is no panel to read a warning off, so
`--export` says it out loud: a look the edit names and the directory
has not got is a line on stderr naming the file that was written
without it. The file is still written. An export is not worth failing
over a look, but a batch that quietly drops one and exits zero is how
a folder of pictures comes out wrong and nobody finds out.

**Three film presets, and no LUT files at all.** `Muted Slide`, a
transparency look — saturation out of the picture, a hard shoulder on
the parametric curve, a cool shadow wheel; `Warm Negative` — soft
contrast, lifted blacks, a warm shadow and highlight wheel, fine
grain; `Red-Filter Mono` — the black and white section at the Red
filter's weights and a strength of 1.2, a dark sky, light skin, coarse
grain. Honest names, not the makers' trademarks, as §78 asks. Every
one of them is the parametric controls the editor already has, so none
needs a file to exist and every number in them stays a slider the user
can move. They are ordinary `.gcp` files, embedded in the binary and
written into the preset store the first time there is no store — after
that the store is the user's, and one edited stays edited and one
binned stays binned.

**And every one of them was wrong at first, in a way reading the
sliders could not show.** The first cut of Muted Slide set
`blacks: -0.12` on top of a parametric shadows of -0.15 and a tone
shadows of -0.15, and put its cyan on a grading wheel at 0.18. Every
one of those numbers looks modest. Rendered, they were not:

| | mean saturation | pure black |
|---|---|---|
| a seaside frame, no preset | 0.399 | 0.0% |
| Muted Slide, as first written | 0.505 | 53.4% |
| a night frame, no preset | 0.516 | 0.0% |
| Muted Slide, as first written | 0.509 | 78.1% |
| Red-Filter Mono, as first written | 0.057 | 38.5% |

A preset called *Muted* that raised saturation and took half the frame
to pure black. Two lessons, both about scale.

`blacks` is a black *point*, not a shade: `-0.12` puts it at 12% of
mid grey, which is 0.0216 linear, which is 0.16 encoded — 41 of 255,
clipped to zero. A photograph has most of its pixels in the shadows,
so that is not a look, it is a hole. The black point is a per-picture
decision and a shipped preset has no business making one: all three
now leave it at zero and get their depth from the parametric curve,
which is a curve and cannot clip.

A grading wheel at 0.06 — a shift of 0.012 in Oklab a and b — reads as
nothing on the slider and is the strongest thing in the preset,
because it puts chroma into the neutrals, which is most of a frame,
where there was none. Measured section by section on one frame,
against a plain render at 0.399: the grading alone gave 0.531, the
`color` section's saturation of -0.35 alone gave 0.280, the light 0.430,
the curves 0.403, the mixer 0.385. The wheel was doing three times
what the desaturation was undoing. It is now 0.03 in the shadows and
0.015 in the highlights, against a saturation of -0.45.

Where they landed, on two frames and three presets (mean saturation is
`(max - min) / max` per pixel, which a change of brightness alone does
not move):

| | seaside, sat | black | night, sat | black |
|---|---|---|---|---|
| no preset | 0.399 | 0.00% | 0.516 | 0.00% |
| Muted Slide | 0.362 | 0.02% | 0.317 | 0.04% |
| Warm Negative | 0.248 | 0.00% | 0.402 | 0.00% |
| Red-Filter Mono | 0.057 | 6.2% | 0.083 | 7.3% |

Red-Filter Mono's remaining black is the red filter doing its job on a
night sky, and its remaining saturation is the rounding of a neutral
to eight bits.

**So the test renders them.** Asserting on the signs of the sliders is
what let all this through; the test now builds a synthetic frame,
finishes it through each preset, and measures what came out. The frame
has to be the right shape for that: sixty-four levels spread evenly
over nine stops, which is roughly a photograph's histogram on a log
axis, at sixteen hues and four chroma levels weighted low. Both halves
were learned the hard way — a uniform grid of the encoded cube, which
it was at first, has only a ninth of its pixels in the deep shadows
and let the black point through unnoticed, and without the pale
colors the grading wheel did not move it at all. With both, the old
numbers fail it: 40.6% to black, and 0.599 saturation against a plain
0.436. A structural test beside it still checks that each preset
carries exactly the sections it lists, since they are hand-written
JSON and a misspelt key is silently a default.

**What it costs.** The stage is a matrix, a clip, an encode, four
node reads and three multiply-adds, a decode and a second matrix per
pixel. On the CPU: a 6 MP finish is 81 ms without a look and 106 ms
with one (32 threads, a 17-node table at strength 0.8), so about 4 ns
a pixel, against a 0.7 s develop the finish already sits behind. On
the GPU it does not register against the rest of the shader — the
viewport draws at the same rate with a look on. A 64-node table is
2 MB of half floats on the GPU; a 17-node one, 40 KB.

**Left out.** No LUT file is committed: the tests build their own
tables, and the §18 check used one written for the occasion. The look
table and the tone curve inside a DCP (§122) are still applied by
nothing — they are a look by this line's definition, and turning one
into a `Lut3d` is a small piece of work that wants its own turn.
A soft gamut map in place of the clip, which would want a rendering
intent and a design of its own. Per-adjustment looks: a mask carries a
`Look`, and a LUT under a mask is a different question. Nothing on the
CLI: `greycard-cli`'s outputs are scene-referred, with no tone curve,
so a display-referred table has no place in them.
