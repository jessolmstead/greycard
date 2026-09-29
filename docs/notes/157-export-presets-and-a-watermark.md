# 157. Export presets and a watermark (2026-09-23)

Roadmap v0.6.0's line, built in wave A by an opus author and read by an
opus reviewer who re-exported every measurement.

**What a preset holds, and where.** An export preset is a named copy of
everything the export sheet asks: format, JPEG quality, long edge (a
named size or a typed custom one), color space, embed-profile, output
sharpening, the if-it-exists policy, a new metadata choice, and the
watermark. It is not a develop preset. It says how a picture is
written, not how it looks, so it lives in `settings.json` (as
`export_presets`, a list of `{name, ...}`) next to the sheet's last
choices, not in a sidecar or the preset directory. The sheet's own
fields moved into one `sheet::Sheet` that the settings flatten under the
`export_*` keys they always had, so an older settings file still reads
and a preset in the file uses the same keys as the sheet. The settings
also keep `export_preset`, the preset last chosen or saved. The sheet
opens on that name and on the fields as they were left, which may have
been edited since. Save as, Delete and choosing a preset write the
file straight away rather than waiting for the window to close. A
snapshot or batch run never writes it: its state carries no settings
file to write to.

"(edited)" is worked out, not tracked. `Sheet::same` compares both
sheets normalized: quality rounded, and ignored for PNG and TIFF; a
custom edge ignored under a named size; sliders compared at 0.1; the
watermark's fields ignored when it is off, and the image path ignored
for a text mark. So moving a field away from the preset and back clears
the "(edited)" flag. Every sheet property has a `changed` handler in
`app.slint` that calls one Rust callback.

**Names.** A name is one name in any case. Save as over a name that
exists in another case replaces that preset and takes the new casing,
and the button reads Replace rather than Save while the typed name
matches one. "None" is the picker's entry for no preset, so no preset
can take it in any case, and a preset called "none" in an older file is
never found. `--export-preset NAME` takes the exact name first. If
there is none, it takes the single preset that matches ignoring case.
Two that differ only in case (possible in a hand-edited file) are an
error that lists both rather than a guess, and so is a name the settings
do not have. The format still comes from the `--export` path's
extension when it has one, and `--long-edge` and `--on-exists` still
override the preset.

**Metadata policy.** All (as before), No edit (EXIF and the XMP without
`greycard:Edit`, so a picture can be handed on without the recipe), or
None (no EXIF and no XMP; the ICC profile still follows the embed
toggle). Location, camera and time stay out with None.

**A mark with nothing to draw is refused.** Text or Image chosen with
blank text, no PNG chosen, or a PNG that is not there fails the export
with the reason (`Mark::check`). It is never skipped, because a
watermark that silently goes missing lets an unmarked picture out,
which is the one thing a watermark exists to prevent. In the sheet the
check runs before the file chooser: the sheet stays up and the status
line says why. On the command line the run exits 1.

**Placement and scale.** The mark's *width* is a share of the export's
long edge, as is the margin: `width = round(size x long)`, `margin =
round(margin x long)`. One of nine positions puts it the margin in from
the edges it names and centers it along the others. This is Lightroom's
"proportional" model, and it is the one that lands the same on a 2048 px
export and a full-size one. A text mark is sized by its ink width, not
its font size, so a longer line is set smaller. That is the price of
one model for text and image, and it is what keeps a signature from
running off a small export. On a narrow picture the long-edge size can
be wider than the short side (a 1:2 crop with Size at 50 %), so the
mark is then shrunk to fit inside the margins on both axes. It is
scaled once by how far over it is, then shrunk a pixel at a time to
absorb the text's rounding. Its box is also clamped onto the picture.
Text is measured at 200 px and set at the size that makes its ink the
asked width. ab_glyph does not hint, so the scaling is linear and one
measurement is enough. The PNG is premultiplied before its Lanczos
resize so its soft edges keep no fringe from whatever color its clear
pixels hold. Its colors are read as sRGB and taken into the export's
space; white and black text are the same numbers in every space.

**Compositing space.** The mark is laid over the finished, encoded
pixels after the resize and output sharpening and before the file
encode. It is never drawn in the viewport. The blend is plain `over` on
the encoded values: `out = under x (1 - a x opacity) + mark x a x
opacity`. So a white mark at 50 percent over a background at 40/255
comes out at 147.5/255, half-way in the numbers the file holds. This is
the usual default for layer blending in image editors and how browsers
composite by default (CSS blending in sRGB), though each editor's watermark
code was not checked. It is also what "50 %" looks like to the
eye: the encoded values are close to perceptually even, so half-way in
them reads as half-way. Blending the same mark in linear light would put
that pixel near 189/255 (linear 0.021 half-way to 1.0 is 0.51, which
encodes to 0.74), a mark that looks much stronger than its slider says.
Doing it on the 8- or 16-bit values also means the mark is not
sharpened, tone-mapped or resized twice.

**Font.** No font rasterizer or font file was in greycard's own crates,
but `fontique` (Slint's font discovery: fontconfig, Core Text,
DirectWrite), `ab_glyph` and `unicode-script` were already in the tree,
so all three came in without adding a package to `Cargo.lock`. The
mark's text is set in the system's sans-serif family, falling back to
system-ui, regular face: Noto Sans on this machine. A character that face
does not draw is looked up by its Unicode script in fontique's fallback
chain, which asks fontconfig, Core Text or DirectWrite for that
script's family. Each glyph is set in its own face on the first face's
baseline, with kerning only between glyphs of one face. "写真" on this
machine comes from Noto Sans CJK KR (fontconfig's pick for Han with no
language given, so Japanese-specific glyph forms are not guaranteed). A
character no face draws as an outline fails the export and names the
character, for example "📷 (U+1F4F7)". That includes a color emoji,
whose font is bitmaps ab_glyph cannot draw. A missing-glyph box in a
delivered picture would be worse. Slint's own text stack cannot be
reached from the export worker. Bundling a font would have meant a
binary file and a license line for one feature. It is the obvious next
step if exports have to match across machines exactly.

**Measured** on `3G0A4650.CR3` (EOS R6, 5472 x 3648) through `--export
--export-preset` in a headless editor. Boxes come from differencing
each marked JPEG against the same export with the mark off (ImageMagick,
threshold 10 %):

| export | size | mark | box (w x h + x + y) | expected |
|---|---|---|---|---|
| text, bottom right, 20 %, margin 2 %, 70 % | 2048 x 1365 | "© Jess Olmstead" | 411 x 52 + 1596 + 1271 | width 410, right and bottom 41 in |
| same, full | 5472 x 3648 | same | 1094 x 138 + 4269 + 3401 | width 1094, right and bottom 109 in |
| PNG 400 x 200, top left, 12 %, margin 2 %, 80 % | 2048 x 1365 | logo | 246 x 123 + 41 + 41 | 246 x 123 at 41 |
| same, full | 5472 x 3648 | logo | 657 x 329 + 109 + 109 | 657 x 329 at 109 |

Width over export width: text 0.2007 and 0.1999, logo 0.1201 and
0.1201. The 2048 text is one pixel wider than the target because of JPEG
ringing at the threshold; the pixel test on raw buffers holds it to a
pixel. The logo's orange (255, 136, 0) at 80 % over sky at (213, 213,
221) measured (246, 151, 43) against an expected (247, 151, 44).

The review's cases through the same CLI: an empty text mark and an
image mark with no PNG each exit 1 with "the watermark's text is empty"
and "no PNG is chosen for the watermark", and no file is written.
"📷 photo 写真" exits 1 naming U+1F4F7. "写真 © Jess Olmstead" exports
with the Han characters in Noto Sans CJK and the Latin in Noto Sans. The
logo at 50 % of the long edge with a 10 % margin on a portrait crop
(1365 x 2048, where it would be 1024 wide) comes out shrunk to fit
between the 205 px margins. Measured by its orange: x 205 to 1159 (955
wide, which is 1365 less two margins) and y 785 to 1262, centered
vertically at Left.

Timing: `export::mark` logs its own wall time at debug level. Three full-size exports of each mark in the release build, each a fresh
process, so the text mark's figure includes the first font lookup. Text:
13, 37 and 15 ms. Image: 9, 4 and 5 ms. The exports around them took
1.3 to 1.7 s, with one 4.1 s outlier while other builds were loading
the machine. The mark is about 1 % of an export.

The sheet: at the default 1500 x 950 window, with a custom size and a
text mark it fits whole with no scrolling, because the position grid
now sits beside the Size, Margin and Opacity sliders instead of on a row
of its own. At 1280 x 720 the header (title, preset picker) and the
Cancel and Choose file buttons stay fixed, and the choices between
them scroll in a ScrollView with a visible scrollbar.

**Tests.**
- Settings and presets:
  - the settings round trip with presets and marks, and an old file with no preset keys
  - `Sheet` to export settings, where an empty mark is still asked for and refused
  - normalized equality
  - save, replace and delete by name, in any case, with None refused in any case
  - lookup: exact, a single case-insensitive match, ambiguous and unknown names refused, a stored "none" never found
- Watermark pixels:
  - the nine placements
  - an image mark checked by pixel: box, margin, and opacity at 50 % measured against the ground, everything outside unchanged
  - a text mark checked by pixel: box, margin, and its fully covered pixels at 50 % opacity half-way to white, nothing brighter
  - scale invariance across 2048 and 6000 wide for both kinds
  - a mark too big for a 1:3 strip shrinks inside its margins at all nine positions, for an image and for text
  - an unknown character is refused with its code point, and a Japanese line takes a second face from the fallback with every character drawn by a face that has it
  - 16-bit blending
  - text tests skip, printing why, on any platform without a font
- Export:
  - the metadata policy for all three policies in JPEG, PNG and TIFF, and the ICC profile in each
  - a render with a mark that moves only the mark's box
- The sheet, driven through its callbacks:
  - choose, edited, back, save as, delete, with nothing written without a settings file
  - a pick, a save and a delete each written to a temporary settings file at once, with the file's other keys kept
  - an empty text or image mark keeps the sheet up with the reason on the status line
- The CLI: `--export-preset` parsed as the CLI parses it, through `opening_sheet` and the batch settings to a written JPEG checked by pixel, with `--long-edge` overriding and an unknown name refused.

**What is left.** Output sharpening is already per preset (it is one
of the sheet's fields). Batch export over a selection waits on
multi-select: a preset is the natural thing to hand it. Export naming
(a pattern like `{name}-{size}`) and a destination folder are not part
of the sheet yet, so they are not part of a preset either. One mark
per preset: text and a logo together would need a list of marks, and
the model extends to that. A text mark has no drop shadow or
background plate, so white text over a white sky is lost. The font is
the system's and cannot be chosen yet, and color emoji cannot be drawn.
Neither can a script whose fallback font has no outlines here.

**The review.** The compositing, the color conversion, the alpha and the
metadata all held under reproduction, and every mark box in the table
came out to the pixel. What the first cut got wrong was at the edges:
an empty text mark or an Image mark with no PNG chosen exported an
unmarked file and reported success, the one failure a watermark exists
to prevent; the sheet with a custom size and the mark rows no longer
fit 950 px and the Flickable it scrolled in drew no scrollbar, so at
720 px the Size, Margin and Opacity rows and both buttons sat below
the fold unseen; a preset saved as "none" hijacked the picker's None
entry; two presets differing only in case could both exist and the CLI
picked one silently; a character the system font lacked was drawn as
a box; and a mark on a narrow crop ran off the left edge. All are
above as the design now is. The second pass reran every CLI case,
viewed the exports and the sheet at both window sizes, and confirmed
the mark stays inside its margins on a 1:3 strip at every position.

**Landed after, on the first CI run.** The macOS runner failed the
scale-invariance test: it wanted the text's ink within a pixel of a
fifth of the width at two sizes, and a glyph's pixel bounds round
outward at both ends, so the width lands a pixel or two over depending
on the face (Noto Sans gives 411 for 410; Helvetica evidently more).
The text is now set a second time from what its own size measured,
and the test says two pixels at either size, in pixels. The runner's
log kept no panic text, so the Mac's actual number is not known.
