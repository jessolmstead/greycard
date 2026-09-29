# 102. A patch shows its shape, and a fill says what it is doing (2026-09-19)

The roadmap's line: "The Fill retouch tool works but you can't see
where you have drawn, there's no indication of such." True of every
patch, but felt only with Fill. A heal or a clone shows its result at
once, so the stroke's shape was implied by the picture changing under
it; a fill shows nothing until the model answers, which is a second
or more, and if the model was missing or failed it showed nothing
ever, with a line on stderr the only trace. The viewport's only marks
over a patch were the chosen one's pins, its center and its source,
and a fill has no source.

**The outline.** The chosen patch's edge is now drawn over the
picture while the Retouch tab shows, the patch under the pointer
included since a press chooses it: a disc for a spot, the swept path
for a stroke, in the overlay's thin blue at `#7fd0ffaa`, and inside
it the line where the feather begins at a quarter of that, the pair
the pointer's own cursor already draws. It is for every method, not
Fill alone, since the shape is the same thing and a heal's soft edge
is as worth seeing. It hides with the tab, not with the tool, so a
fill still on its way keeps its mark after Esc.

The shape is a contour, not an offset. The first cut walked the
polyline's offset, arcs on the outer side of a turn and miters on the
inner, and the review found what that cannot do: a stroke scribbled
back and forth over a blemish, rows a radius apart, which is the Fill
gesture, is a union of overlapping stadiums, and an offset walk drew
chords across the inside at every reversal (four to seven
self-crossings on a hand-drawn wiggle). So `outline::contours` samples
`distance_to_polyline` (now public in `greycard-edit::retouch`, the
engine's own coverage distance) on a grid over the stroke's bounding
box, cells a quarter of the radius and never finer than a
hundred-and-twenty-eighth of the extent, and marches squares at zero
of `d - r`, crossings interpolated along the grid's edges and linked
edge to edge into closed rings; the two saddle cases go by the cell's
middle. That is exact for any self-overlap, a looped stroke comes out
as an outer ring and a hole, and the feather line is the same routine
at the inner radius. The rings are contoured in the picture's units
once per patch (kept in `State::patch_shape` under the points, radius
and feather they were made for) and each frame maps their vertices
through a `ViewMap`, the view's mapping read from the panel once
rather than per point as `source_to_view` reads it, so it turns,
flips, zooms and pans with the picture; the commands string is one
buffer written with `write!` and set only when it changes.

Tests: a spot's polygon is a circle to a sixteenth of a cell with the
area of one to 3%; a straight stroke is a stadium of the right area;
the review's zigzag (four points, rows 0.01 to 0.02 apart at a radius
of 0.02) and a hairpin give rings that never cross themselves nor
each other, checked by testing every pair of non-adjacent segments
for a proper crossing, with every vertex at the radius from the
polyline to half a cell, the hairpin's feather line at a tenth of the
radius too; a stroke drawn round a circle gives exactly two rings at
the radii of the outer edge and the hole. The old "no vertex beyond
the radius" test passed the chords, which is why the crossing test
is the one that stays.

**The status.** `Ai::fill` now answers `Result<Vec<f32>, NoFill>`
with `Missing` (no store, or the model not in it) and `Failed(why)`
in place of a bare `None`; `fill_kept` says whether a patch's fill is
made already and `fill_available` whether the model is in the store.
The worker sends `Outcome::Filling` with the patch's name before it
runs the model for a fill it does not have and can make, and the
status line reads "developing... the fill model is at work on Fill 3"
while it does, over the busy bar that was there already. The
develop's `FillReport` then carries what was made and in how long,
what was left as it was for want of the model, and what failed and
why; the developed status appends them as the denoiser's report is
appended: ", Fill 1 and Fill 3 filled by the model in 8.5 s",
", Fill 1 left as it was until the fill model is fetched", ", Fill 1
left as it was: the fill model failed (…)". A missing model opens the
download sheet, as a missing denoiser tier does, unless it was
declined this session; before, only choosing the Fill tool offered
it, so a sidecar with a fill opened on a machine without the model
said nothing. Declining now says what is lost for the model in
question ("without the model a fill is left as it was"; the denoisers
get their own line too) rather than the mask model's line for all.

Fetching the model from that offer then develops again: `Fetched`
sends a develop when the model is LaMa and the edit has a Fill patch,
as it did already for a denoiser tier the edit waits on. And the
worker's kept patched picture, reused whenever the retouch is
unchanged, now remembers whether every fill in it was made; one with
fills left unmade is served only while the model is still missing,
so the develop after the fetch makes them rather than serving the
unfilled picture until the patch is nudged.

`--patch N` chooses a patch on opening, as `--show-mask` chooses an
adjustment, for a snapshot of its shape on the Retouch tab. Checked
on 4Z4A1023.dng with a sidecar holding a four-point Fill stroke and a
Heal spot: the snapshot shows the V of the stroke with its feather
line inside it over the sea and "Fill 1 filled by the model in 6.4 s"
(a debug build) on the status line; the same with an empty
`XDG_CACHE_HOME` shows the outline, ", Fill 1 left as it was until
the fill model is fetched", and the LaMa download sheet; and the
review's zigzag on the picture turned, flipped and angled five
degrees shows one ring round the whole scribble with no line across
it. The moment of "at work on" was not caught in a snapshot, which
waits for the develop; it goes through the same delivery as the fetch
progress. `--screenshot` writes the renderer's viewport alone and
shows no overlay, so a shape wants `--snapshot`.
