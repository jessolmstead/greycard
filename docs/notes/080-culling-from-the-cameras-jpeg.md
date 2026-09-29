# 80. Culling from the camera's JPEG (2026-09-19)

Choosing a frame decodes and develops it, two seconds at 45 MP, and
§57 only stopped the old picture wearing the new file's look during
that time. Arrowing through a card of five hundred frames at that
rate is not culling, and culling is the first job a shoot goes
through, before any of the library index (§72) matters.

The picture to cull from is already in the file. Every raw carries
the maker's JPEG, full size on the GFX bodies and the Canons, and
the filmstrip decodes it in 40 to 110 ms for its thumbnails (§35).
That is what Lightroom's embedded previews and Photo Mechanic use,
and why they feel instant. Nothing has to be developed to decide
whether a frame stays.

**The mode.** A loupe on the embedded JPEG: arrow keys move, the
viewport shows the preview fitted and turned by the orientation tag,
and no develop starts. The JPEG is display-referred sRGB, so it
bypasses the working pipeline and goes through the monitor profile
only, as an encoded texture: the second small path in the shader
that §57 chose not to add for its own problem, earned here.

- *Prefetch.* The next few frames in the arrow's direction decoded
  ahead of the key at screen size, a window of a dozen or so either
  side kept, so a switch is under a frame and memory stays bounded.
  The screen-size previews go in the cache keyed by hash once the
  library's cache is, so a second pass through a shoot costs nothing.
- *Rating keys* in the loupe: stars, pick and reject, the meta
  section's (§72). A filter to show only picks or hide rejects closes
  the loop.
- *1:1 on the JPEG* for a focus check; the embedded picture is full
  resolution on these cameras. The camera's sharpening is on it,
  which is fine for sharp against soft.
- *Compare*: two or four frames side by side for choosing between
  near-duplicates, at the loupe's level rather than the grid's (the
  lightbox item is the grid).
- *Leaving.* Enter or any slider starts the real develop, and the
  picture swaps from the camera's rendering to ours when it lands.
  They differ by design; the swap is visible, not blended.
- *Rejects out.* A move-rejects action that puts the flagged files
  and their sidecars in a folder beside the shoot. Never a delete.

Waits on the meta section and nothing else: the preview decode and
the orientation handling exist. A few days of UI work.
