# §322. X-Trans, step 1: Markesteijn's demosaic (2026-10-15)

Until now every develop of a Fujifilm X-Trans raw was refused at the
door of `prepare`: the decode was right (a 6x6 pattern, levels, crop,
matrices, as-shot white), but nothing past it knew a mosaic that was not
2x2 Bayer. This section is the first step of X-Trans support: a demosaic,
the refusal lifted, and every stage either taking an X-Trans mosaic or
stepping aside on purpose. The learned denoiser is not touched.

## The demosaic

`develop/xtrans.rs` is a port of RawTherapee's `xtrans_interpolate` and
`xtransborder_interpolate` (`rtengine/xtrans_demosaic.cc`), which is
Frank Markesteijn's algorithm as Dave Coffin published it in dcraw,
adapted to RawTherapee and optimized by Ingo Weyrich. One and three
passes; the three-pass refines green and works in eight directions
instead of four. We kept the reference's tiling (114-pixel tiles stepping
by 98), its constants and its order of work. The tiles of one row form a
band, a band runs on its own rayon thread, and each band writes rows of
its own, so the output is split between them without a copy. The outer
eight sites (eleven for one pass) take the reference's 3x3 weighted
mean.

Four departures, all in the file header:

- Homogeneity is judged in YPbPr for both pass counts. RawTherapee uses
  CIELab through the camera matrix for three passes; darktable's port
  of the same algorithm uses YPbPr throughout, and our demosaics take
  no camera matrix. The samples are white-balanced camera RGB, which is
  what YPbPr assumes.
- The homogeneity maps are zeroed for each tile, as dcraw has them.
  RawTherapee's maps share memory with the same tile's YPbPr buffer,
  and in the tiles along all four edges of the frame the 5x5 sums read
  entries the maps never computed, the bytes of YPbPr floats. With
  three passes that moves the sites eight to ten rows or columns in
  from the frame's edge, by up to 0.43 on the X-E4 frame. One pass's
  border of eleven covers them.
- The border's weighted mean falls back to a 5x5 plain mean for a
  color the 3x3 does not hold (a frame corner can be all green), and to
  the site's own value where neither does; the reference divides by
  zero there. A frame under 32 pixels either way is all border.
- The result is clamped to `0..=max(1, largest sample)`, as our other
  demosaics are, so reconstructed highlights above 1.0 survive.

The same corner case was in the engine's bilinear demosaic, which is
general over RGB patterns but left a color it found no sample of at
zero: `bilinear_pixel` now widens to 5x5 for that color. A Bayer frame
of two pixels or more never reaches the widening.

### The crop's phase

The crops of the sample frames start at (12, 21) and (6, 13), not on
the pattern's period. `balanced_samples` already hands every stage the
pattern shifted to the crop's origin, and the port reads its layout
(the solitary green's phase, the rows of two greens, the hexagons)
from that shifted pattern, so nothing more was needed. A test mosaics a
picture at every one of the 36 phases and checks the demosaic gives the
same picture, moved. On a frame of one tile the two agree to 1e-5.
Where two tiles meet they differ by up to 0.007 on a scene that swings
0.6 over thirty pixels. This is the reference's own seam: a tile
computes red and blue from its third column in, and the 2x2 greens two
columns further in read what was left at zero. Through the derivatives
and the 5x5 sums that reaches the eighth column, the first a tile
writes. With the maps zeroed, the
port is bit-exact against RawTherapee's code on the whole X-E4 frame.
A test pins it to RawTherapee's output on a synthetic scene of four
tiles at a shifted phase, for one and three passes, by 72 sampled
sites (the seams and the border among them) and two sums over the
frame. The frame's four corners are left out, where our border
fallback departs on purpose.

`CfaPattern::xtrans()` is the layout from its own origin and
`is_xtrans()` accepts any of its 36 shifts. A mirror or a transpose of
X-Trans is one of those shifts, and rawler's pattern for the X100VI,
X-E4 and X-T5 is the canonical one moved by (2, 2). `Display` says
"6x6 (X-Trans)" only for X-Trans now; another 6x6 pattern is spelled
out.

## Methods on X-Trans

Each demosaic is a method of one mosaic. `DemosaicMethod` gains
`Markesteijn1` and `Markesteijn3` ("markesteijn-1", "markesteijn-3").
Bilinear is defined on both mosaics: every 3x3 window of X-Trans holds
all three colors, so it is correct there, and soft. The other five are
Bayer's. `DemosaicMethod::for_pattern` lists a mosaic's methods with
its default first, and `default_for` names that default: AMaZE+VNG4 on
Bayer as before, Markesteijn's three passes on X-Trans. The bench gives
three passes more than one at every setting we ran (+0.55 dB CPSNR on
Kodak, +0.28 on McMaster, in sRGB) for about twice the time, which on a
40 MP frame is 0.6 s against 0.3 s. `demosaic_cfa_with` refuses a
method its mosaic does not take, both ways, in words ("amaze is a Bayer
demosaic and this is a 6x6 (X-Trans) mosaic").

A first build mapped every Bayer method to Markesteijn on X-Trans and
kept the menu as it was. We rejected that: a menu that lists a method
and runs another is a lie the user cannot see through.

**Resolving a choice of the other mosaic.** An edit can still carry a
method its frame's mosaic does not take: an older sidecar's default
(amaze-vng4) on an X-Trans raw, or a preset made on a camera of the
other kind. One function decides what runs:
`DemosaicMethod::for_mosaic(pattern)`, or `resolve(Mosaic)` by the
mosaic's kind alone. It keeps the method where the mosaic takes it and
otherwise gives the mosaic's default. The engine resolves the settings
once, at the start of a develop (`resolve_settings` in `prepare_with`
and `demosaic`). Everything after it reads the method that runs: the
noise a dual demosaic measures for, the demosaic, and the method
`Developed` reports. The editor's develop, the headless export, the
thumbnails and the reduced develop all go through it. A choice that
gives way is warned of in the log, and an export names it among what
it left out ("the demosaic: rcd is not one for this raw's mosaic;
markesteijn-3 instead"). The exception is the engine's own default,
which is what an edit stores when nobody chose. Without it every
X-Trans frame under an edit from before X-Trans would warn, and its
export would exit as having left something out. The edit is never
rewritten by the resolve, so opening a file never dirties its sidecar.

The command line is stricter, since its flag is a choice made for the
file at hand. `--demosaic` left out is the raw's mosaic's default,
with nothing logged. A method the mosaic does not take is refused, and
the error names the methods it does take.

**Knowing the mosaic before the pixels.** The menu has to be the open
frame's from the first moment. A first build read the mosaic from the
decoded frame of the last open. While a new frame decoded, the menu
offered the previous frame's mosaic, and a click stored a Bayer method
on an X-Trans file.

rawler's metadata does not carry the pattern. Its description of the
image does, and its "dummy" read fills that in without decoding a
sample. That holds for every format but DNG, whose decoder reads the
image data whatever the flag: 23 ms on our DNG sample against a
millisecond for its tags. `decode::mosaic_path` therefore reads a DNG's
mosaic from its raw IFD's `PhotometricInterpretation`,
`CFARepeatPatternDim` and `CFAPattern`, and every other raw's from the
dummy read. `decode::probe_and_mosaic_path` reads it on the same
decoder as the EXIF. `Stance` carries the mosaic too, from the read it
already makes.

What the panel's file is, and what is known of its mosaic, are set in
one place: `browser::panel_frame`. It is called wherever the panel's
file changes (an open, a pick, an offline frame, leaving culling). It
reads nothing. It asks, in order, the session's own record (the
worker's word, a decoded frame), a stance already read, and the
library's row. Nothing on the window's thread opens the file. When
none of them knows, the menu rests with the hint "reading the raw".

The worker reads the mosaic beside its decode, on a thread of its own
(`worker::decode::open_told`), and sends `Outcome::Mosaic` as soon as
it has it. The window skips that read when it already knows the file.
`Opened` carries the decoded frame, whose pattern says the same, so the
order the two land in does not matter. `browser::learn_mosaic` keeps
what the worker says and brings the panel up to date when it is about
the panel's file.

The menu ends in one of these resting states: "not a mosaic" for a
linear DNG or a monochrome sensor, "the raw could not be read" when
the open failed before anything said what the raw is, and "no demosaic
for this mosaic" for a pattern the engine does not take.

**Opening is no slower.** The first build of this read put the stance
on the open's critical path. That made every open slower, Bayer
included: the file was parsed twice, and on a DNG the stance costs
about as much as the decode. Measured with the editor's headless
export of copies under target/, at low priority, against master's
build (time from launch to the decoded frame, and to the developed
one, medians):

| file | runs | decoded: master | decoded: branch | developed: master | developed: branch |
|---|---|---|---|---|---|
| X-E4 RAF | 30 | 106 ms | 104 ms | (refused) | 778 ms |
| R6 II CR3 | 7 | 206 ms | 205 ms | 1828 ms | 1823 ms |
| DNG | 7 | 43 ms | 44 ms | 1595 ms | 1590 ms |

All within noise.

The menu lists only the open frame's mosaic's methods and shows the
resolved method selected. A pick goes through the window's
`demosaic-picked` callback, which stores the picked name as the edit's
choice. Clicking the method already shown stores it too. Reading the
panel back, as any other slider does, takes the stored choice and
nothing else. The viewport's "not previewed" check compares the two
edits' methods as they run on the frame's mosaic.

**Sync, copy, paste and presets.** These follow the camera profile's
precedent. The source's demosaic travels as the source frame runs it:
an X-Trans frame's stored amaze, never picked there, goes as
markesteijn-3. The section is then left off any target whose mosaic
does not take the method, and the status line names those frames ("the
demosaic left off IMG_0002.CR3 (a method of another mosaic)"). A copy
and a saved preset resolve their source the same way. A paste and a
preset's apply check each target, the frame on screen included.

A target's mosaic is asked only when the method is not one every
mosaic takes; bilinear goes anywhere. It comes from the library first.
Schema 11 of the library index adds a `mosaic` column (bayer, xtrans,
other, none, unknown). The indexer reads it on its own threads, on the
same decoder as the EXIF for a new or changed file, and on its own for
a row from before the column on the next pass, which reads nothing
else of the file. Following the index's pattern, the step from 10 to
11 is an in-place `ADD COLUMN` rather than a rebuild.

On the six local sample copies (CR3, RAF, ARW, NEF, RW2, DNG), warm, a
fresh index takes 100 ms against master's 92: about 1.3 ms a file, the
image description read. Per format:

| format | probe | probe and mosaic |
|---|---|---|
| CR3 | 0.5 ms | 0.5 ms |
| RAF | 0.4 ms | 1.3 ms |
| ARW | 0.6 ms | 1.8 ms |
| NEF | 0.4 ms | 1.2 ms |
| RW2 | 0.5 ms | 1.5 ms |
| DNG | 1.2 ms | 3.4 ms |

**The upgrade's pass.** The first pass over each folder after the
upgrade reads every indexed raw's image description once, to fill the
column. That is 0.3 to 7 ms a file locally (most raws 1 to 2 ms; a DNG
by its tags, about 1 ms). On a network root it is background reading
across the whole library, one small read a file. It runs on the
indexer's threads like the rest of the pass and stops with it between
batches (a test stops it after the first batch and finds the rest left
for the next pass). A file whose description will not read is stored
as unknown and not read again until it changes.

A failed description costs the mosaic alone. A Nikon HE or HE* raw,
whose image this build does not decode, keeps its camera, ISO, date
and lens in the index as before, with its mosaic unknown
(`decode::MosaicRead`).

The read of a DNG's tags is bounded: a repeat past 16 a side, or of
nothing, or with fewer pattern values than the repeat, is decided
before any value past the repeat is read, and is a mosaic of no kind
the engine takes. A damaged file's 65535-by-65535 repeat no longer
asks for eight gigabytes. `CFAPlaneColor`, when the file has one, maps
the pattern's values to colors.

A file the library has not indexed yet is read on the window's thread,
once a session (`decode::mosaic_path`, a millisecond or two locally,
a round trip on a network root). A sync is synchronous on the
window's thread, so moving that read to another thread would mean
making the sync wait on it. Only frames outside the library come to
it. A folder open from a network root before its first index pass
finishes is the case left: its first sync pays one read a frame.

Two paths let a method of the other mosaic through, both on purpose. A
target whose mosaic is not known, neither to the library nor to a read
(a file that would not say), takes the method. A preset applied at
import is not checked: it goes into the sidecar as it is. Both resolve
where it shows. On open the menu shows the mosaic's default selected.
An export develops with the default, names the method that gave way
among what it left out, and so exits 2 from a headless `--export`.

**The default's exception.** The warning and the left-out report pass
over `amaze-vng4`, the engine's default, because that is what an edit
stores when nobody chose, and every sidecar from before X-Trans
support holds it. Without the exception, every X-Trans frame under an
old or untouched edit would warn at each develop, and its export would
exit as having left something out when nothing chosen was lost. A user
who picked AMaZE+VNG4 on a Bayer frame and synced it to an X-Trans one
is not warned; the sync leaves it off that frame.

**The schema.** The two names are new values of the edit's `demosaic`.
A build that does not know them fails to parse a sidecar that holds
one. The repository's way to make an older build refuse cleanly is the
schema version, which every write carries and every read checks
("schema version N, this build reads up to M"). It was raised to 5 for
the virtual copies (§312) and has not shipped: 0.6.0 reads up to 4. So
every released build already refuses any sidecar or preset this one
writes, with words, before it reaches the demosaic. A second bump
inside the same unreleased schema would protect only master builds
from the days between, which §312 says are not a goal, and it would
cost a migration step that moves nothing. We did not bump; the names
are schema 5's, and a test says so. Presets, copy and paste and sync
of the DEMOSAIC section, its history label and the Lightroom import
carry the value as they carry any other.

## What steps aside

The stages with no X-Trans version are gated on a Bayer pattern where
they are called, and an X-Trans frame passes them by:

- **Noise measurement.** `noise::estimate_noise` reads Bayer cells.
  With no noise model, the **hot pixel repair** (which judges a site
  against the model) and the **profiled denoise** do not run either,
  and the dual demosaic's automatic threshold has nothing to read.
- **Chromatic aberration correction**, both the CPU reference and the
  GPU's, which fit per-color shifts on Bayer's red and blue lattices.
- **The capture sharpen's radius measurement**, which reads the
  sharpest diagonal pair of Bayer greens. The sharpen itself runs on
  the developed picture as before, at its default radius when the
  setting is automatic.
- **The learned denoiser**, whose contract is a packed 2x2 Bayer
  mosaic. It is refused on an X-Trans frame before anything is
  prepared or loaded (below), and the engine's develop runs without a
  denoise.

`prepare` logs what an X-Trans develop leaves out (`xtrans_skips`, a
pure function with a test). The defaults (noise measurement, hot pixels,
CA, the radius) go in at `info`. A profiled denoise the user asked for
goes in at `warn`, since what was asked for does not happen.

The editor says so too. The NOISE section on an X-Trans raw carries
the hint "not on X-Trans yet" and its controls rest, as they rest with
"raw only" on a picture. An edit that still asks for a denoise (a sync
from a Bayer frame, say) develops without one. The status line says
"no denoise: no denoiser has an X-Trans version yet", and an export
lists it among what was left out. The worker checks the frame's layout
before choosing a denoiser, so on X-Trans the learned model is never
loaded and nothing is prepared for it. `run_learned` and the command
line's `--ai-denoise` refuse a mosaic that is not 2x2 Bayer before
preparing, naming the pattern. Before this they failed for want of a
noise model after a whole `prepare`, and claimed a fallback to the
engine's denoise, which does not run on X-Trans either. Highlight
reconstruction (inpaint opposed and segments) reads 3x3 neighborhoods
and runs on X-Trans. The segment pass takes darktable's column phase
for X-Trans blocks (2) where it had the Bayer rule; the X-E4 frame's clipped lamp and sky come
out neutral.

## The reduced develop

The editor's reduced develop bins the mosaic in place of the demosaic.
At two photosites an X-Trans cell can be four greens. At three, every
cell at every phase holds all three colors: five greens and two each
of red and blue where it sits on one of the pattern's 3x3 blocks, and
one and three where it does not. `XTRANS_BIN` is 3, `cell_of(pattern)`
gives the cell, and `bin_for_cell` keeps the rule that the binned long
edge must keep 2048, now at three times 2048. A 26 MP or 40 MP X-Trans
frame bins to a third. A bin asked of an X-Trans frame that is not a
multiple of three is raised to one, and `Reduced::bin` says which was
taken. `bin_bayer` is now `bin_mosaic`. The editor's `bin_under` takes
the cell from the frame's pattern.

We chose the bin over a demosaic followed by a downscale because it is
the simplest correct answer and keeps the reduced develop's point, a
tenth of the time. Its cost is that a cell off the pattern's blocks has
its red and blue a photosite off center, a third of a binned pixel. We
did not align the bin to the blocks, since that would move the whole
picture up to two thirds of a pixel against the full develop. The
reduced check (`reduced_check.rs`, six cases a frame on the three
samples) passes its limits: the median ΔE at 2048 is 0.48, the worst
mean 2.2 and p99 16.1, both on the X-E4's foliage, against limits of 3.0
and 18.

## Measured

The bench mosaics the Kodak and McMaster sets with `--pattern xtrans`.
It runs the engine's methods by their names, every one the pattern's
mosaic takes for "all". On X-Trans it refuses up front the options that need the CA
correction or the noise estimate (`--aberrate` without `--no-ca`,
`--denoise`, `--ai-model`, `--dual-contrast noise`).

| set, domain | bilinear CPSNR / ΔE | 1-pass | 3-pass |
|---|---|---|---|
| Kodak, sRGB | 29.27 / 4.94 | 35.65 / 2.59 | 36.20 / 2.47 |
| McMaster, sRGB | 31.01 / 4.19 | 34.37 / 3.01 | 34.65 / 2.95 |
| Kodak, linear | 28.97 / 5.04 | 33.24 / 3.18 | 33.63 / 3.07 |
| McMaster, linear | 30.48 / 4.37 | 31.57 / 3.89 | 31.75 / 3.84 |

Zipper falls from 43% (bilinear) to 18% (one pass) and 16.5% (three
passes) on Kodak in sRGB. For scale, the same sets as RGGB Bayer
give AMaZE 39.22 on Kodak and RCD 36.12 on McMaster. An X-Trans mosaic
samples red and blue more sparsely in places, and these numbers are in
line with that.

On the X-T5's 40 MP frame (7728x5152, 32 threads, five runs, least):
bilinear 0.065 s, one pass 0.31 s, three passes 0.61 s. The editor's
whole develop of that frame, headless, was 1.2 s, and its export took
2.5 s.

The three sample frames (X100VI at ISO 500, X-E4 at ISO 640 with
clipped highlights, X-T5 at ISO 3200) develop through the command line
and the editor's headless export. Their color and orientation match the
camera's JPEG. At 1:1 we found no maze, no colored grid that a wrong
phase would leave, and no fringing on hard edges. The ISO 3200 frame
shows the worm-like chroma noise Markesteijn gives on noise. Nothing
denoises it until the noise measurement has an X-Trans version.

## What is left

- Noise measurement on X-Trans, which brings back the hot pixel repair,
  the profiled denoise and the dual threshold with it.
- Chromatic aberration correction on X-Trans, CPU reference first, then
  the GPU's.
- The capture sharpen's radius from an X-Trans mosaic. RawTherapee
  measures it there.
- A learned denoiser contract for X-Trans: the network's input is a
  packed Bayer mosaic, and X-Trans needs a packing of its own and
  training data.
- A GPU demosaic, if the CPU's 0.6 s becomes what a develop waits on.
- rawler's DNG decoder reads the samples on a dummy read. The fix
  belongs upstream; until then `Stance` on a DNG costs a decode (a
  turn's read, not an open's), and our mosaic read goes by the tags.
- A sync's first read of frames the library has not indexed yet, on
  the window's thread (above).

