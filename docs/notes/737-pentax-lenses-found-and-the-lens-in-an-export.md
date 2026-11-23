# §737. Pentax lenses found, and the lens written into an export (2026-11-22)

Issue 197: a K-3 III DNG and a K-1 PEF got no lens correction, and
exports carried no LensModel.

**Bodies under two names.** lensfun files Pentax's Ricoh-era bodies
under "Ricoh Imaging Company, Ltd." with "Pentax" as the English
maker. We kept the first `<maker>` and compared first words, so
rawler's "Pentax" never met them. A body now matches under any of its
maker's names, each dropped from both names before comparing. Against
every rawler camera, clean and EXIF names, the change only adds:
thirteen Pentax bodies, the Epson R-D1 and its s and x, and a few
Kodaks. A reviewer's own sweep of 5,241 names agreed: 170 newly
found, none moved to another entry. Lens names keep the first maker
only. Dropping an alias from both sides loses a shared word, and on a
sweep of 138,673 lookups that let a Lumix tie with Meike's own lens.
The aliases only add the lens-make bonus. The body matters past the
name: it picks the calibration by sensor size and fills the picker.

Lens matching is version 3, so pictures kept from edits corrected by
the file's profile are made again.

**A Pentax DNG's lens.** The camera writes no LensModel; the lens is
in the maker note in DNGPrivateData, as LensRec, which rawler's PEF
decoder reads and its DNG decoder does not. The fix belongs in
rawler, and a patch is written for it. Until it lands we read the
same tag through the decoder's root IFD, resolve it in rawler's own
lens table (reading the note's entries by hand, within the data, so a
hostile count allocates nothing), and fill the metadata as rawler
fills a PEF's. When rawler does it, this goes with no change in
behavior. Pentax's lens ids are shared with some Sigma and Tamron
lenses, so a name read this way can be the wrong one of two.

**The lens in an export.** An export copied the source's EXIF, so a
file naming no lens exported none, and a lens picked by hand was
never written. The lens found or picked now goes into the metadata as
LensModel, and LensMake where known, only where the source's tag is
blank. Every writer (JPEG, PNG, WebP, TIFF, the linear DNG) carries
it from there. A lens picked from the database is made by its maker's
English name (Pentax, not Ricoh Imaging), and lensfun's Generic by no
one. A compact's lens taken from its body is not named; the body says
it. A pick is named whether its correction is on or off, and a
source's own LensMake stays beside a LensModel we fill; both are on
the roadmap as the owner's call.

**Still open.** lensfun has no FA 35mm F2, so the PEF finds its body
and no profile. greycard does not open .PEF files at all; only the
command line's `info`, `lenses`, and `develop` read them.
