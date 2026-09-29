# 91. Slint 1.18, and wgpu 30 (2026-09-19)

The wgpu major the viewport is pinned to (§14) moved for the first
time. Slint 1.18's FemtoVG renderer is on wgpu 30 and keeps
`unstable-wgpu-29` for Skia alone, so the feature, the module, the
selector and the `GraphicsAPI` arm all change number together and
the rest is whatever 30 broke. In two hundred and forty call sites it
broke one thing: `get_mapped_range` returns a `Result` now instead of
panicking, which is four lines, two let-chains that already asked
whether the map had succeeded and two readbacks that take a
`context` and a question mark; the staging buffers are unmapped on
every path as before. The other real break, `VertexState::buffers`
becoming a slice of `Option`, the code escapes by having no vertex
buffers at all. The descriptors the changelog warns about were
either already written in 29's `Option` form (the pipeline layout's
bind groups) or end in `..Default::default()`. Naga 30 wants
`@interpolate(flat)` on integer varyings and rejects `enable`
directives it cannot honor; the viewport shader has neither, so the
math was not touched. Nor was the limit: 16384 and the storage
buffer stay. The proof is that the export is pixel-identical to
1.17's, down to everything but the timestamp it stamps itself with,
and the `--zoom 1` screenshot is a byte-identical PNG, so §18's
0.12-of-255 agreement between the viewport and its CPU reference is
the same number it was. A whole-window snapshot differs in the
panels and the strip, and the reviewer measured that it is not a
layout shift: the tiles sit in exactly the same place, and what
differs is femtovg 0.27's thumbnail resampling and harfrust 0.12's
text shaping, 0.07 percent of the pixels above the strip, a toolkit
minor's cost. Nothing outside Slint's and wgpu's trees moved in the
lockfile; the CLI's tree is byte-identical.

Two things about the strip came with the upgrade and neither needed
code. 1.18's changelog says Wayland's first frame was rendered at
the wrong size and is fixed, so §89's intermediate-width pass was
worth re-measuring: a `debug()` in the `changed width` handler
prints 560 then 1500 under 1.18 and 560 then 1500 under 1.17,
identically, so the pass is still there and the reset to zero before
revealing stays. And the Flickable now forwards a press straight
through when there is nothing to pan; with four files the content is
exactly the strip's width and `content-x` is zero, so the new
`can_pan` is false and the thumbnail hears the press at once rather
than after the Flickable has finished wondering. What the upgrade
asked in return was a rename: `viewport-x` and its three siblings are
deprecated aliases for `content-x` now, and the nine sites in the
strip took the new names, with the resting positions of the
remembered frame measured identical under both, so the build prints
no warning.
