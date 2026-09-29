# 51. What an export says about itself (2026-09-17)

The other half of §40. An export now carries, beside the source's
EXIF and its own, an XMP packet: `xmp:CreatorTool` and
`xmp:ModifyDate`, and under a greycard namespace `Source` (the file
it was made from, by name), `Output` (what the sheet asked for, in
words: "JPEG quality 88, long edge 1600, sRGB, output sharpening
Standard") and `Edit`, the whole edit as the sidecar writes it,
schema version and all. The source's name is in the EXIF too, as
`OriginalRawFileName`, the DNG tag that any TIFF structure holds.
So a picture says where it came from and can be made again, which
is what Lightroom's `crs:` block and darktable's history in XMP are
for; the engine takes no schema, so the edit crosses it as an opaque
string.

**How.** The image crate's encoders take EXIF and a profile but no
XMP, so the JPEG and the PNG are encoded to memory and the packet is
spliced in: a JPEG's APP1 after the APPn segments the encoder wrote,
a PNG's `iTXt` chunk keyed `XML:com.adobe.xmp` after `IHDR` with its
CRC computed here; the TIFF, written whole since §40, gets tag 700.
A JPEG's segment holds sixty-four kilobytes, so a packet that would
not fit is written without the edit and with a note saying so; a
brush-heavy edit could reach that, and a PNG or a TIFF has no such
limit. Tested by round trip through the image crate's decoders for
the JPEG and the PNG and by the bytes for the TIFF (the image crate
sizes the tiff crate's buffers by the picture, and refuses a packet
longer than a tiny test picture's pixels; exiv2 reads all three).
Checked with exiv2 on a real export: `Xmp.greycard.Edit` of 2.6 KB
beside `Exif.Image.OriginalRawFileName`. The CLI's previews carry
the packet without an edit, since it has none.
