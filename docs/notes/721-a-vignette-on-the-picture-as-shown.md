# §721. A vignette on the picture as shown (2026-11-19)

**Asked.** Our Vignette (§32) is an exposure falloff in the scene, so
a bright edge stays bright: a clipped sky in a corner is still white
at two stops down. A tester's view is that a vignette is for effect,
not optics, and should darken what is on screen, as most editors'
post-crop vignettes do (discussion 170).

**Decided.** The Vignette carries a place, `Vignette.place`: `Scene`,
the default and the falloff as it was, or `Display`. The panel's row
is Style, with Natural and Overlay.

- **Where.** The display vignette acts in `finish_pixel_display`,
  after the display curve, the color curves, and the look table, and
  before the output sharpen, the output matrix, AgX's rail, and the
  grain. Every finish passes that one spot, so exports, thumbnails,
  and the viewport's shader agree. Masks act before it. The clipping
  warnings and scopes read after it.
- **Darkening** multiplies the display-linear working-space value by
  `2^stops`. A uniform scale commutes with the output matrix, so hue
  and chromaticity hold. A color past the output gamut darkens from
  its unclipped value.
- **Lightening** is `v / (k + v(1 - k))` per channel, `k = 2^-stops`:
  the same stops in the deep shadows, rolling off to white, nothing
  clipping. Its pole is below zero, where `v <= 0` takes `v / k`. Per
  channel it softens color: a sky at +2 loses about half its chroma
  and turns about 10 degrees. The roll-off on max(R, G, B) holds hue
  and reads as a saturation boost; a paint toward white washes the
  shadows grey. We kept the fade toward white, the look a positive
  post-crop vignette has elsewhere.
- **The edit.** A scene place is not written, so existing sidecars
  and develop keys are as they were, and the schema is not bumped. An
  older build reads a display vignette as the scene's and drops the
  place at its next write. A place this build does not know reads as
  the scene's (`Place::Unknown`), where it would have made the whole
  sidecar unreadable.
- **Carried.** The place travels with the Amount in the copy sheet,
  presets, and sync, so a pasted number is the same look.
- **Lightroom.** `PostCropVignetteStyle` 1 (Highlight Priority) maps
  to the scene, 2 (Color Priority) and 3 (Paint Overlay) to the
  display; absent, the scene as before.

**Measured.** With the place at the scene, 26,220 CPU renders, 1,776
viewport draws, and 874 edits' JSON and keys are byte-equal to the
build before. The shader is within 2 levels of the CPU over 192
display cases, 3 over the scene's as before.

**Left.** A tonal control, a lift of the middle, a center, and the
Amount's reach at a high Feather stay on the roadmap.
