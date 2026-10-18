# §352. The soft proof named on the picture (2026-10-18)

The soft proof is one S away with the pointer on the picture, and
it is not remembered between runs, by design: a proof is a question
asked of a picture, not a state of the window. The gamut warning
was on by default, and it paints an exact mid grey wherever a color
falls outside the proof's profile. So a stray S on a Rec.2020
working space proofed to sRGB turned the saturated parts of every
picture flat grey, in patches that moved with the exposure, and
nothing on the screen said why. It cost us an hour today, and the
scopes, which bin the painted picture, grew a spike at mid grey
that the new readout faithfully reported on every channel.

Two changes. A pill in the top left corner of the picture reads
"Proof: sRGB", or the profile file's stem, with "· gamut" when the
warning is painting; it is the only part of the overlay that takes
the pointer, its tooltip says S turns the proof off, and a click on
it does the same, the shortest way out for someone who hit S by
accident. It is hidden in culling, where the camera's JPEG is shown
as it is. And the gamut warning is off by default, as Lightroom's
is: a proof without it still shows the proof's rendering, and
someone printing turns it on in the SOFT PROOF section, where it is
remembered. Existing settings files keep whatever they hold.

The clipping lamps live on the scope, not on the picture, so the
badge could not sit beside them as the roadmap line said; the
picture's corner is where a proof belongs anyway, next to what it
changes.
