# §451. Version 2 profiles in exports

*2026-10-29.* An Adobe RGB JPEG from greycard looked flat in Windows'
photo viewer while Lightroom's of the same frame looked right there,
and Photoshop read both (issue 53). The two embedded profiles have
the same primaries and the same gamma; ours was ICC version 4 as lcms
builds one (parametric curves, `mluc` text, a `chad` tag), Lightroom's
version 2 (`curv`, `text`, `desc`, the white point D65 with the
colorants adapted to D50 and no `chad`). The viewer ignores the
version 4 profile and shows Adobe RGB values as sRGB, which is
exactly the flat, desaturated look reported.

Exports now embed version 2.1 profiles built the classic way: lcms's
own colorants, the version set before any tag is written so lcms
picks the version 2 tag types, one `curv` shared by the three
channels, a single gamma entry for Adobe RGB and a 1024-entry table
of the sRGB function for the spaces under it, `wtpt` D65. The display
table's own profile (`finish_icc`) stays lcms's version 4, since only
lcms reads it. Through lcms the new profiles transform within 2e-5 of
the old, exactly for Adobe RGB, and our own reader recognizes them.
Exif already says sRGB for sRGB and Uncalibrated for the rest, as
Lightroom does. The Windows viewer itself is checked by hand.
