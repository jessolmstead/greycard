# 115. The proxy develop, deferred and redrawn as a placeholder (2026-09-20)

§90 put a binned proxy develop for the fit view first among the
things that would make a fanless laptop feel quick, and the v0.3.0
list carried it as the headline. Pulled from this round, for two
reasons that were asked about directly.

**It answers a measurement nobody has taken.** §90 is an
extrapolation from pinning threads on a desktop, and its own last
word was to build on an M-series machine and take the table again
before any of it. A Mac is now on hand. The per-slider cost that
hurts here is the sharpen, and the GPU sharpen addresses that with
no change to what the viewport shows; the open-time cost is the
proxy's real case, and it is not known yet.

**It makes the fit view disagree with the export**, which this
project has held against (§18 and its checks since). Not where one
would first look: binned quads against AMaZE are invisible at fit,
since the screen is already taking four photosites to one display
pixel and the half-photosite offset between the channels in a quad
is an eighth of that. The real differences are elsewhere. Highlight
reconstruction reads clipped channels, and binning a clipped green
with an unclipped one gives a value under the clip point that is
wrong, so the clip mask would have to be taken per photosite and
binned with max or blown highlights come back tinted. The sharpen
and the denoisers cannot run on the proxy meaningfully, so the fit
view would show the frame without them. And zooming past the
threshold would swap pipelines, with sharpening and noise appearing
and the retouch patches resampled: Lightroom's pop, which people
notice.

**If it is built, it is a placeholder, not a mode.** The worker shows
the proxy only while a full develop is pending and replaces it with
the downsampled full develop when that lands. At rest the fit view
equals the export again, the pop becomes a one-time refinement after
a base change instead of a property of the zoom, and on an Air the
first picture is up in half a second instead of five. The same
worker seam serves it, and it pairs with the embedded JPEG that the
culling mode (§80) wants as the cheapest possible first frame on
open. The roadmap line now says so and waits on the Mac timing.
