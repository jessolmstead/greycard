# 152. A picture that is not a raw takes no baseline and no curve (2026-09-23)

Issue #1: a JPEG opened 0.8 stops brighter than the file. The baseline
of §141 was added wherever the exposure becomes a gain, and a picture
goes through the same finish as a raw's develop, so it was brightened
too. Worse, and older than the baseline: since §50 a picture has also
gone through the display curve, which is made for a scene. A JPEG
already has a curve, whatever rendered it applied one, so its tones
went through two, the mid-tones lifted and the top compressed a
second time.

The finish now knows what it is handed: `finish::Source`, `Scene` for
a raw's develop and `Display` for a JPEG, PNG or TIFF, carried on the
worker's `Base`, the UI's state and the viewport's `View`. A `Display`
picture takes no baseline, and in place of the curve a clip at white.
The Light sliders' shape (contrast, the tone equalizer, whites and
blacks) still acts on it, in the same linear working space, since
those are edits and not a rendering. The shader's `curve` uniform
takes a third value for the shape followed by a clip. The export, the
droppers' `pick` and the mask models' preview all read it, so what
the viewport shows, what a dropper reads, what a model is shown and
what is written agree.

Checked on a 640×480 JPEG through the release editor with the
display profile off: the export against the file is 0.5% RMSE (14%
before), and the 1:1 viewport the same, which is the JPEG's
re-encoding and eight-bit rounding.

Left as it is: whites aims at display white, which for a scene is
where the raw clips plus the baseline, 3.27 stops over grey; a
picture's white is 1.0, 2.47 stops over grey. So on a picture the
slider moves the white point further than its number says near the
top of its range. It works, and it can be given the picture's own
white if it is found wanting.
