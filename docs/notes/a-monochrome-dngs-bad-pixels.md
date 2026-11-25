# A monochrome DNG's bad pixels

A DNG's `FixBadPixelsConstant` and `FixBadPixelsList` (§513) were left
undone on a monochrome frame (§668), said in the log: the repair reads
a mosaic's pattern, and a monochrome frame has none.

The plane is now given to the same repair as a pattern of one color,
one photosite wide. A bad photosite takes the median of the good ones
among the eight beside it, or within two or four where none is nearer;
on a mosaic the reaches stay two and four. The lists' placement, the
stored value a constant is tested on, and the order with the gain maps
are the mosaic's, and the opcode's Bayer phase is not read for either.
Mosaics are repaired as they were.

The repair runs on the one plane, before it becomes three equal
channels, so before the noise is measured and the hot pixels are
repaired. A linear frame's lists are still left, said in the log.

No file we have carries these opcodes on a monochrome frame: three
Ricoh GR IV Monochrome DNGs have none. The tests are on made-up frames
and a DNG the test writes. The recipes of the kept pictures are not
raised, since only a monochrome DNG with such a list develops
differently.
