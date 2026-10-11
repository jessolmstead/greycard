# §285. Thumbnails from the edit, part two: the reduced develop, its queue, its check, the key in the index (2026-10-10)

The second step of the order in the thumbnails-from-the-edit design
(§270), built: a reduced develop for a frame edited without being opened, the
queue that feeds it from the thumbnails' pool, the check that holds it
against the full develop, and the develop key as a column of the index.
A sync, a preset or a paste over the selection, a save joined from
another machine's copy, and a folder opened cold with its edits on disk
now fill their frames' pictures in the background, behind the camera's
thumbnails; a frame on an offline root shows its own edit's picture by
the key its row carries. Part one (§278) is the open frame's pictures;
the lightbox is still part three's.

The review of the first cut found a loop, a key trusted too far, and a
check that measured the wrong thing: a learned mask whose model is not
in the store had the frame decoded and developed again every second or so for a picture that
could never be kept; a row's key made under a look or a lens database
since moved was trusted, so a refit, or a cache wiped with the lens
database in it, left a frame on its old picture or on the camera's for
good; and the check's 360 was a second resample of the full picture
where both develops make theirs from their own 2048. Each is fixed
below, with the smaller findings beside them: the grain, the yield, the
open frame, and two recipes for the two develops' pictures.

## The reduced develop

`greycard_core::develop::reduced` is the engine's part: everything
before the demosaic runs on the mosaic in full, as `prepare` has it
(the levels, the noise measured, the hot pixels repaired, the gains, the
highlights reconstructed, the chromatic aberration corrected), and the
mosaic is then binned 2x2 into a linear picture at half the size in
place of the demosaic (`bin_bayer`: each cell's red, its two greens
averaged, its blue, whatever the pattern's phase), then `finish` as it
is: the matrix, the profile's map, the orientation. A 24 MP frame comes
out 3000 by 2000, a 45 MP one 4096 by 2732 (2732 by 4096 upright); a
frame whose half would fall under 2048 on the long edge is not binned
(`bin_for`), and a linear frame has nothing to bin. `Reduced` carries the picture, the bin and the
full develop's oriented size.

Why the bin comes after the prepare and not before it: binning a
clipped green with an unclipped one gives a value under the clip point
that is wrong, so the highlights have to be reconstructed on the
photosites; and the CA correction reads the mosaic's own channels at
their own offsets, so it has no meaning on an RGB picture. Both cost
what they cost in the full develop (the CA is the larger), and both are
then exactly the full develop's. What the bin replaces is the demosaic,
which is most of the develop's time, and the pixels after it are a
quarter as many.

The editor chooses the bin itself (`reduced::bin_under`, through
`develop_reduced_by`): binned while the frame, as the edit's crop and
straighten leave it, keeps twice 2048 on its long edge, and demosaiced
as in a full develop under that, so the 2048 picture is never brought
up from fewer pixels (the check, below, says how many and what it
costs).

The editor's part (`greycard-ui/src/reduced.rs`) is the rest of the
pipeline on the half-size picture, as `develop_job` has it after the
base, with these rules per op:

- The capture sharpen, the learned denoiser and the profiled denoiser
  are left out: each is measured on the mosaic's own pixels and means
  nothing at half size. The reference in the check leaves them out
  too; the honest gap below says what a real export is from the
  thumbnail with them on. Since the pictures are not quite the
  export's, they are kept under a recipe of their own (below), and the
  frame's own develop replaces them when it is opened.
- The grain stays. It is the finish's, laid on by `render_framed` at
  2048 on both develops, and placed by the output's own pixels
  (`grain.at` of the pixel's place over the output's size), so the
  same output size is the same grain. The first cut left it out of the
  reduced develop alone, and part one's pictures carried it: two
  pictures under one key differed by it. Now both make the picture at
  the size an export at 2048 of the full develop has, to the pixel
  (`edited::fitted_size`: the geometry's frame of the full develop as
  `geometry::apply` cuts it, then brought to 2048 as `export::fit`
  brings it; `export::fit_exact` resamples to it). Before that, a crop
  rounded at half the scale could land the reduced picture a row off
  the export's, and a grain placed by the output's size then fell
  somewhere else entirely: that, and not the grain itself, is what
  moved the reviewer's case 0 (a crop and Dehaze at full negative on
  the R5) from a mean of 0.42 to 0.85 with grain on. No raise of
  `DEVELOP_RECIPE`: part one's pictures already carried the grain.
- Texture and Clarity (`local_contrast`) take their radii as fractions
  of the long edge already (`radii`), so they scale with the bin on
  their own, down to Texture's minimum of two pixels: at 6000 pixels
  Texture's fraction gives 1.5 on the half-size picture and the minimum
  makes it 2, a third larger in the frame's terms (3 pixels against 4).
  Clarity's grid step follows its radius as it does at any size.
- The dehaze fits on a copy reduced by `ceil(long / 1536)`. On a 6000 or
  an 8192 pixel frame the full and the half picture come to the same
  copy (4 and 2, 6 and 3), but not where the factor is odd: the A7 IV's
  7008 is reduced by 5 to 1402 and its half by 3 to 1168, the R7's 6960
  the same way, so the windows are a fifth larger a fraction of the
  frame on the reduced develop. The check's Dehaze cases on those two
  bodies sit with the others (below), so the effect is under what the
  airlight's own difference does; it is left. The airlight is read off
  slightly different pixels (binned against demosaiced), which is where
  the check's largest means come from.
- The CA correction runs on the mosaic in full, before the bin; nothing
  to scale.
- The lens correction is made for the picture's own size by the
  profile (`correction(width, height, shot)`), so it scales. The
  defringe's radius is the one option in pixels, and is divided by the
  bin.
- The guide plane is made at its fixed 1024 texels from the half-size
  picture, in the picture's own coordinates, as the worker makes it
  after the lens.
- The retouch's patches are in units of the picture's width and apply
  as they are. A Fill is the model's and runs on the open frame alone:
  a picture with one is not kept (the open frame's rule, part one).
- The masks, the guide plane and the vignette are placed by the frame
  in `export::render_framed`, as the open frame's pictures are: the
  finish is `edited::make` unchanged (the geometry, a resize to 2048,
  the export's finish with no output sharpening, sRGB, the turns taken
  back out, the made sizes shrunk from the 2048), with the half-size
  picture as its source.
- A learned mask is read from the mask cache on disk alone
  (`Ai::cached_raster`, the slot `cached_path` names, checked by the
  full develop's aspect, a row either way taken, so a raster made for
  the open frame and one made from the halved mosaic both serve); no
  model is loaded on the pool's threads. Before anything is decoded,
  the making asks whether each learned shape can be had
  (`reduced::masks_ready`, `Ai::raster_ready`): in the cache; to be made
  by a model that is in the store; or not until a model is fetched.
  One whose model is missing ends the making there (`Why::NoModel`).
  One to be made is asked of the worker once a key (`Job::MasksFor`),
  the lowest thing the worker takes: it decodes the frame, develops it
  reduced for the picture the models see, makes each learned shape's
  raster as an export would and puts it in the cache. The rasters go
  in the cache the open frame's masks are read from: made from the
  reduced picture, which the models see at their own 2048 anyway, they
  serve the frame when it is opened too. Until then the frame shows
  the camera's JPEG. A look not in the look directory is not kept
  either, as in part one, and is asked after before the decode too.

The mask loop the review found: with no Sky model in the store and a
Sky in the sidecar, the first cut developed the frame, found the raster
missing, asked the worker, whose making failed and said "kept" anyway,
and the answer cleared the frame's tried mark: 29 decodes and develops
in 32 seconds, for good. Now a frame whose making ends without a picture
is kept as tried under its key (`Reducer::tried`, the key the making was
under, not one read after it) and not wanted again under that key; the
worker's making lets it go only when every raster was made
(`masks_done(path, true)`); otherwise, and for a missing model, the
frame waits for a fetch (`Reducer::waiting`), and a model fetched lets
every waiting frame go and has its cells ask (`models_arrived`, from the
fetch's own thread). A raster the worker made that still does not read
back is not asked for again (`masks_made`). The test
(`a_mask_whose_model_is_absent_is_tried_once_and_made_when_it_arrives`)
has a Sky with no model: one try, nothing decoded, not wanted again, the
camera's picture; the model fetched, the worker asked once; the worker
short, still tried; the raster in its slot, made and kept.

A picture file (a JPEG, a TIFF) is its own base: it is turned, reduced
by the same whole bin when it is large, and goes the same way from the
lens on.

The develops run on one rayon pool of the reducer's own, shared by the
ones in hand (`Reducer::threads`: a quarter of the machine's cores, two
to eight; eight of thirty-two here), so the reduced develops together
take a quarter of the machine, and the window's develop, which has the
thumbnails' pool held while it runs, finds the rest free.

## The queue

The thumbnails' pool (`thumbpool`) gained a third tier under the
thumbnails and the local previews: the edits. After every thumbnail it
delivers (a hit or a making), it asks `Edited::wants_develop` of the
frame: held as edited, under its key (read from its sidecar the first
time a picture of it is looked up), not the open frame, with a cache to keep pictures in,
and not every picture of the key kept already. A frame that wants one
is queued; the queue is ordered as the thumbnails are (`want`: the
cells shown first, the rest outward), cleared with the folder, and
begun only when no thumbnail and no preview can be, never while the
pool is held, and never on the last thread (a thread is always left for
a thumbnail). How many at once is asked as each is begun
(`reduced::begin`): while a frame is open in the editor, whose develop
is what the user waits on, none until the editor has been quiet for
1.5 s and then one (the yield, below); two while culling or browsing on
a pool of four or more threads, one otherwise. A frame under a root
on a network mount (`mounts`, the roots the watcher's build found
remote, `Worker::set_network_roots`) is developed alone, begun only
with nothing in hand and with nothing begun beside it: its read is the
slow part, and two would only share the link. The window's develop
sets the pool's limit to nothing while it runs (`during_develop`), so
no edit begins meanwhile; the one in hand finishes on the reducer's
threads. The queue keeps a set beside it, so the check for a frame
queued already is not a walk of the queue per thumbnail delivered.

Which frame is open in the editor is the window's to say
(`edited::note_editing`, `Worker::set_open_frame`): the frame on screen
while editing, and none while culling, in the grid, or with no frame,
said on every report of the strip's or the grid's range and on the way
into culling. The first cut set it when a frame was opened and never
cleared it, so a culling session ran the queue as if a frame were being
edited. A change wakes the pool and the worker.

Every path the design named feeds it through the one hook, since each
ends in the frame's thumbnail being asked for again: `sidecar_written`'s
non-open branch and the sync's take and join (`edited::saved`, which
holds the frame as edited and asks for its cell), a preset or a paste
over the selection (each frame's save goes through `sidecar_written`),
and the folder pass that finds a sidecar's hash changed
(`rows::apply_row` → `edited::changed`). A folder opened cold with its
edits on disk is fed the same way by its first thumbnails.

The making (`Reducer::make`) reads the frame's sidecar as the window
reads it, and the sidecar's key wins: when the frame was held under
another (its row's, made under what has moved since, or a save came
between), the frame is settled on the sidecar's (`Edited::settle_key`)
and that is what is made, where the first cut gave up as "the edit
changed meanwhile" and, with the row's key wrong for good, gave up on
every frame of a folder each time it was asked (51 of them after a
cache wipe). A sidecar found reset takes the frame back to the
camera's. Then it asks after the look and the masks, takes the picture
in hand under the key and its recipe (`Edited::mark`), develops, reads
the rasters, checks the key once more (a save during the develop is
`Why::Stale`, and the ask that follows that save queues the frame
again), finishes, and keeps the pictures with `Edited::store_reduced`.
The window hears `Outcome::EditedKept` and the cells ask again.

**Two recipes.** The reduced develop's pictures are kept under
`edited::REDUCED_RECIPE` (0x8002), the open frame's under part one's
`EDITED_RECIPE` (0x8001), the same key in the stamp. A lookup tries
0x8001 and then 0x8002 (`Edited::lookup`), and `Thumbs::newest` takes a
list of recipes, so the picture kept last of either stands for a frame
whose sidecar is out of reach. `all_kept` is either recipe's whole set,
which is what the pool asks; part one's keep asks after its own alone
(`all_kept_by`), so a frame opened with the reduced develop's pictures
kept has its own made from its develop, and `Edited::store` then takes
the 0x8002 entries of that key out. Taking a key's pictures out
(`remove_key`) takes both. Each develop marks its own picture in hand
(`mark` by hash, key and recipe), so a frame opened while its reduced
develop is under way is no longer left without a picture: the first cut
gave up on it as the open frame after the develop, and part one's keep
had skipped it on the reduced develop's mark. Now the reduced develop
keeps what it made (its key read from the sidecar), unless part one's
were kept meanwhile (`store_reduced` keeps nothing then), and part one
makes its own beside it either way. The test
(`the_open_frame_s_pictures_take_the_reduced_develop_s_place`) keeps a
dark picture under 0x8002, sees it in the strip, saves the open frame,
and finds part one's under 0x8001 with the dark one gone.

## The check

`worker::reduced_check::the_reduced_develop_is_the_full_one_within_the_limit`
(ignored; `GREYCARD_SAMPLES`) draws edits from a splitmix64 generator
seeded per case as the GPU ops' sweeps are: a brightening and a
contrast always, and each with some chance Texture and Clarity, Dehaze,
a manual chromatic aberration, the defringe, a straighten, a crop, a
radial or linear mask with an exposure and a shadows shift, the
vignette, a `.cube` look from the look directory (the user's own, when
there is one), and the grain (from a generator of its own, so the other
draws are the same with it as without; `GREYCARD_REDUCED_GRAIN=1` lays
it on every case). The reference is the full develop on the CPU,
finished at 2048 as an export at that size is, with no output
sharpening, and its 360 is that 2048 shrunk by `edited::shrunk`, as
both develops make every kept size. The reduced develop is finished by
`edited::make`, its 2048 and the 360 shrunk from it. The two are
compared in Oklab (sRGB decoded, into the working space, `Oklab::of`),
the Euclidean distance times 100, its mean over the picture and its
99th percentile. The sharpen and the denoisers are off on both sides.

The first cut's reference 360 was an export at 360, a Lanczos resample
of the full picture, against our area average of the 2048: most of its
99th percentiles at 360 (7.8 to 17.6 on the reviewer's run) were the
two resamples putting edges a shade apart, not the develop. Against the
reference's own 2048 shrunk the same way, the same cases at 360 have a
median of 0.37 of the mean they had and 0.30 of the 99th percentile
(0.08 to 0.74 case by case).

Measured in release, 18 cases on each of five bodies, crops drawn down
to a tenth of the frame each way, the sample raws hard-linked under
`target/scratch/review/raws5` (`target/scratch/fix/de-wide-18.csv`):

| | mean, 2048 | p99, 2048 | mean, 360 | p99, 360 | demosaiced |
|---|---|---|---|---|---|
| R6 II, 24 MP CR3, 6000 | 0.00 to 0.51 (median 0.07) | 0.0 to 3.1 | 0.00 to 0.25 | 0.0 to 2.0 | 9 of 18 |
| R7, 33 MP CR3, 6960 | 0.00 to 0.65 (0.21) | 0.0 to 2.9 | 0.00 to 0.25 | 0.0 to 1.6 | 6 of 18 |
| A7 IV, 33 MP ARW, 7008 | 0.00 to 0.98 (0.21) | 0.0 to 4.4 | 0.00 to 0.37 | 0.0 to 1.6 | 6 of 18 |
| R5, 45 MP CR3, 8192 | 0.00 to 0.79 (0.25) | 0.0 to 3.7 | 0.00 to 0.36 | 0.0 to 1.6 | 5 of 18 |
| R5 II, 45 MP DNG, 8192 | 0.00 to 1.49 (0.30) | 0.0 to 7.4 | 0.00 to 0.48 | 0.0 to 2.1 | 5 of 18 |

**Tight crops.** The first fix measured crops down to 40% of the frame
only, and the reviewer found what that hid: of 216 geometries drawn
across the editor's range, 123 left a framed long edge under 4096 (a
24 MP frame cropped under about two thirds, or straightened enough),
and those were binned all the same and their 2048 brought up from the
half-size crop. At crops of 12 to 30% the R6 II's hardest case came to
a mean of 1.87 and a 99th percentile of 11.8 at 360, past the limits.
Now the bin is chosen by the frame as the edit frames it
(`reduced::bin_under`): binned while the framed long edge keeps twice
2048, demosaiced as in a full develop under that, so the 2048 picture
is always brought down and never up. With the draw widened to a tenth,
31 of the 90 cases (31 of the 55 with a crop; none with a straighten
alone) took the demosaic. Those are the export to the last value: with
the sharpen and the denoisers off on both sides, the develop is the
full one, and their ΔE is 0.00. They cost what a develop costs: a
median of 1.6 s for the reduced develop and its finish in the check,
against 0.86 s binned. The median of the means at 2048 over all 90 is
0.21; over the 59 binned, 0.29.

The worst, mean 1.49 and 99th percentile 7.4 at 2048 and 0.48 and 2.1
at 360, is the DNG under Clarity at full strength and Dehaze at +0.84
with a crop that still binned: the local contrast and the dehaze
multiply what little the binned and the demosaiced pictures differ by
at edges, and the dehaze's airlight is read off slightly different
pixels, so the whole frame moves a little by it. A brightening alone is
under 0.2 mean at 2048.

The run before the bin rule, crops down to 40% and every frame binned
(`target/scratch/fix/de-raws5-18.csv`), is what the binned develop does
when it has room:

| | mean, 2048 | p99, 2048 | mean, 360 | p99, 360 |
|---|---|---|---|---|
| R6 II, 24 MP CR3, 6000 | 0.14 to 1.32 (median 0.29) | 1.0 to 10.8 | 0.08 to 0.63 | 0.6 to 6.2 |
| R7, 33 MP CR3, 6960 | 0.13 to 1.63 (0.28) | 0.6 to 8.5 | 0.07 to 0.43 | 0.4 to 2.8 |
| A7 IV, 33 MP ARW, 7008 | 0.17 to 1.51 (0.33) | 0.6 to 8.1 | 0.08 to 0.54 | 0.4 to 2.9 |
| R5, 45 MP CR3, 8192 | 0.14 to 1.23 (0.32) | 0.6 to 6.2 | 0.08 to 0.48 | 0.4 to 2.7 |
| R5 II, 45 MP DNG, 8192 | 0.21 to 2.22 (0.49) | 1.3 to 14.0 | 0.09 to 0.72 | 0.5 to 4.4 |

Its worst (2.22 and 14.0 at 2048) is the DNG under Texture and Clarity
at full strength, Dehaze at +0.92, a crop, a mask, the vignette at its
end, a look and grain, the worst on every body (a mean of 1.2 to 1.6 on
the other four). The grain cases sit where the same cases without grain
sat in the reviewer's run, to a hundredth of a mean on almost every
one: the two develops lay the same grain. The Dehaze cases on the A7 IV
and the R7, where the dehaze's reduced copy is not the same fraction of
the frame on the two develops, have a median mean of 0.29 and 0.28 at
2048, the same as the R6 II's, where it is.

The limits, with headroom over the worst of either run: at 2048 a mean
of 3.0 and a 99th percentile of 18; at 360 a mean of 1.2 and a 99th
percentile of 10; and the median of the means at 2048 under 0.5, so the
typical edit is held close and not only the worst bounded. The test
holds them over 18 cases a body by default (`GREYCARD_REDUCED_CASES`),
and `GREYCARD_REDUCED_MEASURE=1` prints without holding. The widened
run took 5.0 minutes in release.

**The grain's case.** With the grain laid on every case, the reviewer
saw the first cut's case 0 (a crop, Dehaze at full negative, a look) go
from a mean of 0.42 to 0.85 on the R5, 1.81 to 2.16 on the DNG and 0.57
to 1.16 on the R6 II. The crop was what did it: the reduced picture's
frame, rounded at half the scale, came out a row off the export's, and
the grain, placed by the output's size, was another grain. With the
size taken from the full develop the same three are 0.41, 1.81 and 0.57
with the grain on (`target/scratch/fix/de-grain3.csv`), and the cases
with no crop are as they were.

**The honest gap.** With the capture sharpen and the profiled denoise on
in the reference alone (`GREYCARD_REDUCED_HONEST=1`), which is what a
real export is from these pictures:
at ISO 100 they move the mean at 2048 by up to 0.19 and the 99th
percentile by up to 1.7, and nothing at 360; at ISO 1000 (the R5 II DNG)
the noise is what Clarity works on and the denoise is what takes it out
of the export, so under Clarity at +1 the mean at 2048 is 3.21 against
the export, where it is 1.78 against the develop without the denoise.
That gap is not closed by anything here; it is why the reduced
develop's pictures are kept under their own recipe and replaced by the
frame's own develop's when it is opened.

## The key in the index

Schema 9 adds `develop_key INTEGER` to `files`
(`Edit::develop_key` as an i64, 0 for a develop whose edit could not be
read). The migration adds the column and clears no hash: a row the pass
finds edited with no key yet has its sidecar read again once
(`Row::same_sidecar`), and the next pass reads nothing. The summary
makes the key for any sidecar with an edit in it, so a frame counted as
edited at the write by its blend has one too. The lens database the
keys are made with is named to the library once
(`greycard_library::set_lens_database`, from `edited::lens_database`
when the registry is made and again when a fetch moves it), and a fetch
clears every key (`Library::clear_develop_keys`, `Ask::KeysStale` from
the window when the lenses land) for the next pass to fill under the
new name.

The window takes the key with the row (`RowMeta::develop_key`,
`FromRow::develop_key`), but only as a first guess. The index made it
when it last read the sidecar, under the looks, the lens database and
the recipe as they were then, and nothing about the sidecar changes when
a look is refitted or the lens database's cache is wiped, so the pass
never reads it again. The first cut held the frame under the row's key
at once and read nothing: the reviewer refitted a look and the strip kept
the old picture, and cleared `~/.cache` (where the lens database lives)
and every one of 51 edited frames stayed on the camera's JPEG, the pool
giving up on each as changed meanwhile. Now, online, the sidecar's own
key is read lazily, as part one does, the first time a picture of the
frame is looked up, and wins (`Held::row` is kept beside it); the row's
key stands only for a frame whose sidecar cannot be read
(`shows_offline`, a root offline), where it beats the picture kept last
for the content. A row that comes in with another key than it came with
has the sidecar read again. The test
(`a_row_key_gone_stale_gives_way_to_the_sidecar_s`) holds a frame under a
row key made with a lens database since wiped: offline it shows the
row's, online the sidecar's, and the pool makes the sidecar's; held
under a key its sidecar no longer makes, it is settled and made, not
given up.

And the index keeps what its keys were made under, so a cache wipe, a
refit while the editor was away, or a raised recipe heals itself: a
`meta` table (name, value; made by the schema 9 step and by the schema)
holds `develop_keys`, `greycard_edit::key::keys_made_under`, which is
the recipe, the lens database's name and a fingerprint of the look and
profile directories (each file's name, length and modification time;
the contents only of the files an edit names go into a key). The
indexer settles it when it opens the library
(`Library::settle_develop_keys`): another value clears every key
(`clear_develop_keys`) and stores the new one, and the next pass over
each folder reads the edited rows' sidecars again. A lens fetch names
the new database to the library from the fetch's thread before the
window hears of it, and the window's `KeysStale` clears and settles. The
tests: the fingerprint moves with a refit of the same length (its time
alone), with the lens database's name, and with a profile added, and
names the recipe (`greycard-edit`); and the index keeps its keys under
the same value across a reopen and clears and refills them under a
refit, a fetch, a wipe and a raised recipe
(`the_develop_keys_go_when_what_they_were_made_under_moves`).

**A look or a profile written while the editor runs.** A frame's key is
read once a list (`note_all` holds every edited frame as to be read) and
then kept, and the first fix moved frames off a key found only when the
lens database changed. So a camera match fit or refit, a look removed
or renamed, or a table or DCP put in its directory left every frame not
open on its old picture until the list was opened again, and the
index's rows stale until the next launch. Now `Edited` keeps what the
keys it found were made under (`keys_made_under`, the same value the
index keeps), and `edited::looks_moved` asks it again after each of
those: the camera match's fit and refit landing, the look removal and
the rename landing, the LOOK and CAMERA PROFILE sections opened (which
read their directories, so a file put there by hand is caught then), and
each frame opened. When it moved, every edited frame is held as to be
read again (as a new lens database has them), the cells of the frames
in the window ask for their pictures again, and the index is sent
`KeysStale`, which clears its keys and settles what they are made under.
A frame whose key moved shows the camera's until the pool has made it
again; one whose key did not finds its pictures kept. The test
(`a_look_refitted_mid_session_remakes_a_frame_not_open`) holds a frame
naming a look in a directory of its own, makes its pictures, refits the
table, sees the frame still on its old key until the directory is said
to have moved, and then made anew under the new one.

This clears more than it must, on purpose. The value is a listing (each
file's name, length and time), so any look added, touched or refitted,
whichever frames name it, clears every key in the index and has every
edited frame read again. Nothing is made again for that: a key is a
hash of the contents of the files an edit names, so a frame whose look
did not change reads back the key it had and finds its pictures kept.
The cost is a sidecar read per edited frame, in the window's list and on
the index's next pass. And the pictures of a key a frame no longer names
after such a move are orphans, left to the eviction (Left).

## Measured

**The reduced develop per frame.** Release, the ignored test
`the_reduced_develop_of_the_samples_timed`, a brightening, three runs
each, the user working on the machine meanwhile. The test calls the
develop directly, so it ran on the global rayon pool, capped at sixteen
threads (`RAYON_NUM_THREADS=16`), not on the reducer's eight:

| | decode | to the binned picture | lens and detail | finish to the pictures | keep | total |
|---|---|---|---|---|---|---|
| 24 MP R6 II (6000 by 4000, to 3000 by 2000) | 89 to 131 ms | 338 to 384 ms | 79 to 86 ms | 167 to 191 ms | 59 to 62 ms | 0.78 to 0.80 s |
| 45 MP R5 (5464 by 8192 upright, to 2732 by 4096) | 162 to 212 ms | 442 to 452 ms | 102 to 104 ms | 131 to 137 ms | 35 ms | 0.88 to 0.93 s |
| 45 MP R5 II DNG (the same size) | 27 to 44 ms | 435 to 584 ms | 114 to 125 ms | 125 to 258 ms | 36 to 59 ms | 0.76 to 1.06 s |

"To the binned picture" is the prepare in full (the levels, the noise,
the hot pixels, the highlights, the CA correction on the CPU), the bin
and the finish's matrix and orientation; the 45 MP frame's is barely
more than the 24 MP one's, since the CA correction, the largest part,
runs in tiles that the pool covers either way. The full develop of the
same frames on the CPU in the check ran 0.9 to 2.5 s (1.1 to 1.4 s at
24 MP, 1.6 to 2.5 s at 45 MP), so the reduced develop is between a
half and a third of it with the decode, and the pictures are
390 to 660 KB a frame. The design's guess of "a few tenths of a second
past the decode" holds for the develop; the finish (the geometry, the
resize to 2048, the export's finish, the made sizes) is the other
tenth or two.

**A cold folder.** The release editor on a headless weston (`--renderer=gl`),
every `XDG_*` folder fresh under the work tree, over a folder of 52
raws hard-linked from the test set (29 of one content each, 23 second
links to the same content, so the second finds the first's pictures
kept), every one with a sidecar of +1.5 EV written beside it, the
thumbnails' pool at its eight threads, `RAYON_NUM_THREADS=16`. The first
cut, two at once throughout: the first edit's pictures kept at 2.8 s and
the last of the 27 the pool had to make at 20.9 s. Now, an editing
launch (the first frame open, so one at a time once it is): the first
kept at 2.4 s, the last of 27 at 24.5 s, each 0.58 to 2.25 s; a culling
launch (no frame open, two at once): the first at 1.7 s, the last of 28
at 22.3 s, each 0.67 to 4.2 s (the 28th is the first frame, which in
culling is no open frame's and is the pool's). Two at once share one
pool of eight threads, so they buy little: the culling launch drained
in 20.6 s from its first keep, the editing one in 22.1 s. In the editing launch two
did overlap once, at the very start: the window says which frame is
open when it opens it, about a second in, and the queue had begun two
before that as if browsing. The open frame's pictures came from the
worker, as part one has it ("the open frame" in the log). The
captures: `target/scratch/fix/drain-edit.png` and `drain-cull.png`, the
strip once drained, every cell brightened and with its pencil.

**The yield.** The same cold folder with `--time-select 8` (the next
frame opened eight times, a tenth of a second apart, each develop
timed from the step to its frame) while the queue ran, against the same
folder with its pictures all kept (the drained cache kept in its place,
everything else fresh, so the run starts at the same first frame), and
against the folder with no sidecars at all (cold, the thumbnails made,
no reduced develop), runs interleaved so the machine's load falls on
each alike (`target/scratch/fix/yield4.log` to `yield9.log`;
`GREYCARD_REDUCED_AT_ONCE` fixes how many at once whatever the window
does, which also takes the idle gate off, `GREYCARD_REDUCED_THREADS` sets
the reducer's threads):

| | the window's develop, mean of 8 steps | against the drained cache | peak resident |
|---|---|---|---|
| drained cache, nothing behind it | 1616 to 1677 ms | | 1.92 to 1.96 GB |
| no sidecars, the thumbnails alone | 1622 to 1633 ms | +0.2% | 2.05 to 2.12 GB |
| two at once, the first cut's rule | 1847 to 1904 ms | +15.0% | 3.46 to 3.68 GB |
| one at a time, no gate | 1817 to 1907 ms | +12.5% to +13.0% | 2.73 to 3.13 GB |
| one at a time on four reducer threads | 1711 to 1725 ms | +5.9% | 2.85 to 3.02 GB |
| one at a time on two | 1725 to 1726 ms | +6.3% | 2.66 to 3.16 GB |
| **the idle gate, now** | 1659 to 1691 ms | +0.9% | 2.55 to 2.85 GB |

The thumbnails cost the window's develop nothing measurable; the yield
was the reduced develop's, one in hand on its eight threads beside the
window's develop the whole time the editor was worked in. Two at once
against one was only 2.5 points of it and about 0.55 GB of the peak
(an earlier +5% for one at a time was measured against a cold run
with no sidecars, with a six-core job running beside it and another
thumbnail thread count, so it is not comparable; the runs here are
interleaved for that reason, and a cache copied to another place is
not warm at all, since the lens database's place is in every key it
names). Fewer threads halve it at the price of
the queue.

So the queue now waits for the editor to be quiet (`reduced::begin`,
`reduced::IDLE`): while a frame is open in the editor, no reduced
develop begins until 1.5 s after the last develop was asked for or
ended (`Edited::touch`, from the worker's send of an open or a develop
and from the end of each; a slider in motion is a develop asked for
every few tenths), and then one at a time; culling and browsing are as
they were. The pool looks again when the quiet is due (a timed wait,
with nothing else to wake it). Under `--time-select 8` the steps never
leave the window quiet, so only the two begun at launch, before the
first frame was open, were made during them, and the window's develop
is within a percent of the drained cache's (gate runs 1 to 5, less one
taken in a burst of load that hit the run beside it alike; the clean
range is above). The cost is the queue's while the editor is worked in:
the editing launch left alone drained its 27 frames by 26.3 s, the
queue resuming 1.5 s after the last develop landed, against 24.5 s
without the gate. The first frame's develop at launch is 2.40 to 2.55 s
with the gate against 2.18 to 2.25 s with one at a time throughout and
1.77 to 1.85 s with the cache drained: before the first frame is open
the queue counts as browsing and begins two (Left).

The mask loop, again in the editor: the reviewer's folder (two frames,
one with a Sky in its sidecar) with an empty model store, 64 seconds:
the frame was tried once, at 0.3 s, "not kept until a model is fetched",
and nothing of it was decoded after (`target/scratch/fix/editor-maskloop.log`),
where the first cut decoded and developed it 29 times in 32 seconds.

## Left

- The lightbox (part three), which reads the 2048 picture.
- A look or a profile changed by hand in its directory, with no sheet of
  the editor's in it, is caught only when the LOOK or CAMERA PROFILE
  section is opened or a frame is opened (both read the directory and
  call `edited::looks_moved`); until then the frames that name it keep
  their old pictures.
- `Job::MasksFor` waits while a frame is open in the editor and gives
  way between two shapes, but its own decode and reduced develop at the
  start, and a model run once begun, are not interrupted.
- The retouch's heal and clone choose their sources
  (`Retouch::choose_sources`) on the half-size picture, so a patch whose
  source was left to the search may take another source than the full
  develop takes. A patch with its source placed by hand is the same on
  both.
- At launch the queue begins as if browsing, two at once, in the second
  before the window opens its first frame (and before the idle gate has
  a frame to hold it for), and the first develop pays for it.
- The lens database's name is its place on disk and its fetch's time,
  so a cache moved or copied to another place (another `XDG_CACHE_HOME`)
  keys every edit anew and every picture is made again; the index heals
  as for a wipe.
- A frame whose develop is refused (X-Trans, a monochrome sensor) keeps
  the camera's JPEG and the refusal is only in the log, once a key;
  nothing marks the cell.
- A frame's pictures under a key it no longer shows are orphans when the
  key moved under it without a save (a refit, a lens database, a moved
  directory: the frame is read again and does not name the old key), and
  are left to the eviction rather than taken out at once.
- The check runs on the sample raws and is ignored in the normal suite:
  a frame large enough to bin developed in full eighteen times takes
  minutes even in release, and no raw can be in the repo. The normal
  suite holds the reduced develop's parts: the bin against its CPU
  reference (`greycard-core`), and the pool's making of a frame edited
  without being opened, end to end on a PNG.
