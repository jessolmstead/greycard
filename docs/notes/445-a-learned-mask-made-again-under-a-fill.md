# §445. A learned mask made again under a fill

*2026-10-28*

The learned masks, Sky, Subject, Background, Landscape, and the
People's parts, were made on the base: the develop before the
retouch. A Fill that took a person out left the Subject mask holding
them, and a local edit on it drew a person-shaped region on the fill.

## The rule

A shape is still made on the base first. That raster is what a Fill is
measured against: a Fill overlaps when its hole lands on texels of the
base raster at 32/255 or more, on the raster's own grid. When one does,
and that fill is made, the shape is made again on the patched picture,
under a stamp of its own, and that raster is drawn, exported, and
saved. The base raster stays the reference on purpose: a raster made
after the fill no longer covers the removed person, so measured against
it the decision would flip back on the next ask. Only a Fill counts; a
Heal or a Clone keeps what is under it. A Fill elsewhere in the frame
leaves the shape alone.

The patched raster is kept in memory by key, turn, shape, and a print
of the overlapping Fills (their points, radius, feather, opacity, seed,
and fill choice), and on disk in a slot salted with the print. After
each develop the worker recomputes the print for every shape it gave
the panel and queues again the ones whose print changed, behind the
develop and never replacing an ask the panel already queued. A shape
that cannot be made again (no model yet, or a Part whose person is
ambiguous on the patched picture) keeps the base raster under the
print; a model arriving retries the ones that failed. A Part whose
person the Fill removed is empty, written to its slot so a reopen does
not run the People model again. The models' kept work for a picture,
the preview, embedding, scene, and mattes, is held per side, base and
patched, so a shape on each does not rebuild the other's.

The panel lets go of its rasters when another version of the file
opens. A reopen of the same version keeps them drawn, so the masked
adjustments do not blink, and asks again for each from the render,
behind the open; the answer replaces the raster in place and the
worker learns again what the panel holds. Such a re-ask does not turn
Show mask on, and a failed one or the user's own ask of a new shape
takes the key off that quiet list. The mask cache's index runs now
strip the retouch, as the People's search across a set already did,
so its slots hold base rasters.

## Cost

No Fill over a shape: no extra run. A Fill over it: one run on the
patched picture per print, then served from disk. A Fill over it still
unmade: the base raster until the fill lands, then one run.

## Not done

A second run that fails for a passing reason (the GPU lost, out of
memory) leaves the base raster until the fills change, a model arrives,
or the frame reopens. An ambiguous Part's answer lives in memory only,
so a reopen asks the People model again before reaching it. A re-ask's
status line still says "found on ... in ... s".
