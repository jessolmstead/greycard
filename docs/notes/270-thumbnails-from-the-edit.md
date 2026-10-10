# 270. Thumbnails from the edit (2026-10-09)

Every thumbnail today is the camera's embedded JPEG, shrunk to one of
the made sizes (128, 176, 256 or 360) and cached by the file's content
hash and the raw's mtime (§163, §172). The local preview for an
offline root is the same JPEG at 2048 (§210). Nothing of the edit
goes in, not even the crop: a save or a sync redraws a cell's turn and
nothing else. Culling never develops (§123), so a frame brightened to
be judged beside the rest of its shoot still shows dark in the strip,
the grid, the loupe and compare.

The case that decided the design: a run of frames shot well under,
brightened together by a sync or a preset so they can be culled with
the others. So the rule is that a frame with an edit shows its edit,
everywhere culling and the library show a frame, and a frame without
one keeps the camera's JPEG. The index already knows which is which
(its `edited` column), so the rule costs no new state. There is no
marker on a frame whose edited picture has not landed yet, and no
setting to turn this off; Lightroom has neither, and the camera's
JPEG standing in until the develop lands is what the viewport already
does (§134).

## The key

A picture made from an edit is kept under a key of what the edit
makes, not of the sidecar. The sidecar's hash in the index changes
with a rating, and `state_id` (§233) changes with the history, so an
undo and a redo back to the same picture would make it again. The key
is a new `Edit::develop_key()`: a hash of what `same_develop` compares,
the turn and the crop, a recipe number for the pipeline, and the
contents of every file outside the sidecar that the develop reads: the
camera match's look table, a `.cube` or HaldCLUT look, a DCP. A refit
of a look then remakes the pictures of the frames that use it, and
nothing else does.

The edited pictures sit in the thumbnail cache beside the camera's,
under the same content hash with the develop key in place of the
camera's stamp, and the camera's stays: it is the stand-in until the
edited one lands, and the picture again if the edit is reset.

## Two ways a picture is made

**The open frame, for nothing.** When the window's develop lands and
the edit is saved, or the frame is left, the picture the window
already has is shrunk to the made sizes and to 2048 and stored under
its key. Most edits are made this way, so the strip keeps up with the
editing at no cost of a develop.

**A frame edited without being opened.** A sync, a preset over the
selection, a paste, and an edit joined from another machine's copy
(§244) change frames the window never developed. The save path
(`library::sidecar_written`), the sync's take and join, and a folder
pass that finds a sidecar's hash changed each compare the frame's key
with what the cache holds, and queue the frame when they differ. The
pool makes it with a reduced develop:

- The mosaic binned 2×2 into a linear picture at half the size, then
  the pipeline as it is. A 24 MP frame comes out about 3000 pixels
  wide, enough for the 2048 picture and every thumbnail shrunk from
  it. This is the proxy develop the roadmap holds for the fit view
  (§90, §115), built here for its second use.
- What is invisible or misleading at that size skipped: grain, the
  capture sharpen, the denoisers.
- What is measured in pixels scaled with the bin: Texture, Clarity,
  the dehaze's window, the CA correction's radius.
- A learned mask from its cached raster when there is one (the
  rasters are 2048 wide). One that is not cached is made as the export
  makes a missing one, at the lowest priority, and the frame keeps
  the camera's JPEG until it is.
- One or two of these at a time, not the pool's eight: a 45 MP decode
  is several hundred megabytes. Visible cells first, behind the
  camera's thumbnails, and held while the window develops, as the
  pool already is.

The 2048 picture is what the cull loupe and compare show for an
edited frame, and it goes into the local previews too, so an edited
frame on a root that is away still shows its edit. A 1:1 look at focus
still opens the camera's full JPEG: what the eye checks there is
focus, which the brightening does not change.

## The check

This picture is what the lightbox will compare and what a frame is
culled on, so it has to be the export at a smaller size, not an
impression of it. The reduced develop gets its CPU reference in the
full develop shrunk by `export::fit`, and a test that holds them
within a ΔE limit at 360 and at 2048 on the reference set (§150),
across seeded random edits as the GPU ops are checked (§264). The
limit is set from the first measurement and written here.

## The order

1. The develop key, the edited entries in the cache, and the open
   frame's picture kept when it is saved or left. Small, and the
   strip shows edits from the first part.
2. A measurement of the reduced develop per frame at 24 and 45 MP;
   our guess is a few tenths of a second past the decode, so a cold
   folder of 500 edited frames is a few minutes in the background,
   once. Then the reduced develop with its check, and the queue from
   the save, sync and folder pass.
3. The lightbox's larger tiles, which read the 2048 picture.
