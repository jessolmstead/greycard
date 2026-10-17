# §323. Fitting the Lightroom mapper to its exports (2026-10-16)

The Lightroom catalog import (§79, in build) carries the metadata
exactly and the develop settings through the mapper in `lightroom.rs`
(§52), whose scales were chosen by eye: "about where the slider lands
there". The feel track has been measuring those scales one slider at
a time against a set of nine raws with one slider at its limit per
export (§150), and is still short of bases for four of them. An
archive of Lightroom catalogs with the raws and the exports beside
them is the same measurement, made on hundreds of frames, with the
exact settings behind every export read from the catalog rather than
set by hand. This section is how we use it. It is the camera match
(§180) pointed at Lightroom instead of at the camera's JPEG.

**What a frame gives.** For each image the catalog holds the develop
settings as the mapper reads them, and the export history says which
file was rendered from which settings (Lightroom keeps the export's
settings with the image; where a catalog has no export history the
export is paired by name and the current settings are assumed, with
the pairing's confidence said). The raw developed by us with the
mapped settings, registered to the export and compared block by block
in Oklab, is one error per frame, and the settings that frame used say
which sliders the error is about. The registration, the block pairs
and the error are `greycard-match`'s (`register`, `pairs`,
`color::mean_delta_e`); the export is resized and output-sharpened,
so both sides are blurred to the block before comparison, as the
camera match blurs to its block.

**The profile comes first.** An export is Lightroom's profile and
then its sliders. The catalog says the profile per image (Adobe
Color, a camera-matching one, or a creative one), and the archive's
shoots mostly use the camera's own styles, for which we already have
fitted looks (§180, §260). So the comparison runs with that look on,
and the error left is the sliders'; without it the profile's error
swamps anything a slider does. A frame whose profile we have no look
for is reported and left out of the fit, and the count of those says
whether a look is worth fitting from the camera JPEGs first.

**Two steps, measurement before fit.** First the measurement as a
command: catalog, root mappings as the import takes them, the error
per frame, and the error grouped by which sliders are off default on
that frame, so a slider whose frames carry twice the error of the
rest is named. That number alone decides which mappings are wrong
and is worth having before any fitting. Second the fit: frames kept
where the edit is global (no masks, no local adjustments, no
PV2003/2010, a profile we have a look for), and each slider's scale,
and where the shape is wrong its curve, fitted so the error over the
set drops; the exposure, contrast, highlights, shadows, whites,
blacks, temperature and tint, vibrance and saturation, and the tone
curve, in that order of how much they move a picture. Held out by
shoot, not by frame: the edits inside a shoot are one look over
hundreds of frames, so a fit on one shoot that holds on another is
the test that it generalizes, and three catalogs may give fewer
independent points per slider than their frame count suggests. The
measurement says.

**What it changes.** The mapper's scales, by number rather than by
eye, with the error before and after in the notes section of the fit;
the feel track's reference set grows by every global-edit frame in the
archive; and the import's report can say, per catalog, how close an
imported edit lands on average, which is the honest number to show a
tester who is leaving Lightroom. The scales stay constants in
`lightroom.rs`; the fit is a tool that proposes them, not a table
shipped per user, since the mapping is between two programs and not
between a body and a style.

**Not this.** Not a per-frame fit that bends our sliders to one
export: the camera match's rule (§260, one fit over a sample) holds.
Not a port of Lightroom's tone pipeline. Not local adjustments or
masks, which have no counterpart in the mapper and are reported as
skipped by the import.

Waits on the catalog import's part 1 for the reader and the pairing.
The measurement is a day; the fit depends on what it finds.
