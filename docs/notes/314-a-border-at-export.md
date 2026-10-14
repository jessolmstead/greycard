# §314. A border at export (2026-10-13)

Issue #19 asked for a border at export, or a view of the picture framed
on a white background. We built the border. The framed view is left
open (below).

**What it is.** `export::Settings.border: Option<Border>` sits beside the
watermark. A `Border` holds a width and a color. The CPU reference is
`border::add(pixels, width, height, band, color)`. It puts a band of one
color round interleaved RGB at either bit depth, and the picture sits
centered in it, every pixel untouched. `export::border` applies it to
the finished, encoded pixels after the resize and output sharpening and
before the watermark and the encode, in the worker's one `write_export`.
The export path has no GPU half after the resize, so neither does the
border. The viewport never draws it.

**The width is a share of the long edge.** Like the watermark's size, it
is a percentage of the picture's long edge: 0.5 to 25 %, 5 % by default,
rounded to whole pixels and never under one. The same preset then gives
the same proportion at 2048 px and at full size (102 px a side at 2048
by 1365, 274 at 5472 by 3648). A fixed pixel count would make a thin
line on a full-size file and a thick frame on a 1024 px one. The border
is added outside the picture, never cut into it. `long_edge` stays the
picture's long edge, so "2048" still means 2048 px of picture, and the
file grows by twice the band on each axis. The sheet says so in one line
under the Width slider and gives the file's size: "Added outside the
picture; the file is 2252 × 1569." The EXIF pixel
dimensions are the file's, because they come from the rendered size.
Nothing else in the metadata changes, except that the XMP's
`greycard:Output` names the border ("a light grey border 5 %").

**The watermark goes over the border.** It comes second, so its size
and margin are shares of the file's long edge, border and all. Its
margin is measured from the file's edges, so a small margin puts it in
the band. That is where a signature on a framed print usually goes. To
keep it on the picture, make the margin wider than the band.

**Colors.** White, light grey, dark grey, black, and Match. The neutrals
are fractions of the encoded range (1.0, 0.8, 0.2, 0.0). Neutral is the
same numbers in sRGB, Display P3 and Rec.2020, since all three use D65
white, so a white border is 255 in every space. Match is the mean of the
picture's encoded values, channel by channel. It costs one pass over the
pixels, which is small next to the export. We take the mean of the
encoded values, not of linear light, for the same reason the watermark
blends in them: the result looks like the picture's average to the eye.
A linear mean would be pulled toward the highlights. On the R6 sample it
came out at (144, 136, 123) against a picture mean of (143.8, 136.6,
122.6).

**The sheet.** A Border row sits under the watermark's. It holds an Off
box, four color swatches and a Match box, with the chosen color's name
beside them. Picking a color both chooses it and turns the border on.
With the border on, a Width slider and the size line follow. The sheet
holds it as `export_border`, `export_border_width` (percent) and
`export_border_color`. They are flattened into the settings and into
each preset like every other sheet field. `Sheet` is
`#[serde(default)]`, so a settings file or preset from before reads as
no border, and nothing was bumped. With the border off, its width and
color do not count toward "(edited)".

**Headless.** `--export-preset` with a preset that has a border is all a
headless run needs. There is no new flag.

**Tests.** - `border::tests`:
`a_border_grows_the_picture_by_twice_its_band_and_leaves_it_untouched`,
`a_zero_band_is_the_picture`,
`the_band_is_a_share_of_the_long_edge_at_any_size`,
`the_colors_are_neutral_levels_and_match_is_the_mean` - `export::tests::
a_border_goes_round_the_resized_picture_and_the_mark_over_both`: a
16-bit render at 600 px gets a 30 px light grey band, the picture inside
is identical to the render without it, and a bottom-right mark lands in
the band - `sheet::tests::a_border_is_asked_for_only_when_on_and_its_wid
th_is_kept_in_range`,
`sheet::tests::a_preset_keeps_its_border_and_an_older_one_has_none`, and
the border fields added to `settings::tests::the_settings_round_trip` -
`tests/headless_export.rs::an_export_preset_with_a_border_writes_a_file_
larger_by_the_band`: the editor run headless with a preset at 120 px and
a black 10 % border writes a 144 × 104 JPEG with black corners and edges

**Left open.** The framed preview: showing the picture in the viewport
on a white (or any) surround, as it would hang. It would take the
viewport's surround color as a choice, which the canvas color setting
half is already, and, if it should show this export's border, drawing
the band round the picture at the export's proportion. Also open: a
border of different widths per side (a heavier bottom, as on a mat), an
inner keyline, and a color picked freely rather than from the five.

After review: the size line reads "Added outside the picture; the file is
W × H." The reviewer's other notes (a 2 px disagreement between the
sheet's rounding and the band's at exact halves, only in the line shown;
a timing log for the band; an unknown color name in a hand-edited preset
exporting as white with no swatch lit) are left as they are.
