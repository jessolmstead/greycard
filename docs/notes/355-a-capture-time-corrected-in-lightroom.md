# §355. A capture time corrected in Lightroom (2026-10-18)

Lightroom keeps every photo's capture time in its catalog, and Edit
Capture Time changes that column, never the raw. Our catalog import
read the raw's EXIF, so a photographer who had already fixed a
second body's clock in Lightroom lost the fix on import and would
have done it again with §354's sheet. The two testers who asked for
the shift come from Lightroom.

The import reads the catalog's time for each image, in a query of
its own so a catalog without the column still reads, and compares
it with the file's own time, which the import already had in hand:
it probes each file's header for the camera and ISO, and the
capture time rides along, so there is no second read and no decode.
The difference in whole seconds becomes the frame's shift when both
times parse, the difference is two seconds or more (Lightroom
rounds), and it is within the meta's own two-hundred-year limit. A
catalog time equal to the file's writes no shift and, on a forced
import, does not clear one set by hand; a catalog time that differs
wins on a forced import, since that import asks for Lightroom's
state. A catalog time with no readable file time is counted, not
carried. The report gains a line in both voices, "N frames keep the
capture time corrected in Lightroom", with the uncompared count
after it when there is one. A dry run compares nothing, since only
frames written are probed. Virtual copies take their master's
shift, since the shift is per file.
