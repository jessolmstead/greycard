# A monochrome DNG told by its header

`decode::dng_monochrome` says whether a DNG is a monochrome sensor's
(§668) without decoding it, for a consumer that wants to know before it
opens a file, or of every file of a folder. Nothing calls it yet.

It reads IFD0 and up to 32 of its sub-IFDs with the maker note reader's
own bounded TIFF reads, not rawler. `LinearRaw` with one sample a pixel,
or a full-size `BlackIsZero` with one, is monochrome, as at decode
(`rawler_backend::is_monochrome`). A mosaic, or `LinearRaw` with more
samples, is color. A file that holds both, such as a color raw beside a
one-channel picture, is color; rawler takes the first full-size picture
in file order, so the two could part on a file that lists them the other
way round, which we have not seen.

The answer is `None` for a file that has no `DNGVersion`, does not read,
or holds no raw it knows.
