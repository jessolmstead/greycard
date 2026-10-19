# §373. A number that is not finite, and a sync before the rename (2026-10-19)

Two small correctness fixes from an outside review of the code, both
confirmed before they were taken.

## A Lightroom number that is not finite

The Lightroom mapper parsed every number with the standard float
parser, which accepts "NaN", "inf", and "infinity", and `clamp`
passes NaN through. serde_json writes a NaN as `null`, and the
sidecar then fails to load on the null where a float belongs: an
import could write a sidecar the editor refuses. The pp3 reader
already filtered on `is_finite` with a test over the five
spellings; the Lightroom mapper did not, and the catalog import
shares its path.

Now a number that is not finite is absent, in every place
Lightroom's text becomes a float: the mapper's keys, the flat curves
of a catalog table (where a bad word keeps its slot and drops its
own point, where before it shifted every pair after it), the XMP
curve points, a scale given on the command line, and in the catalog
reader the smart-collection rules (a NaN rating, label, or flag
refuses the rule rather than becoming "rating 0", "no label", or
"unflagged") and the file's width and height. Infinity was harmless
in the clamped keys and is taken as absent with the rest, since no
finite value was meant. The crop is the exception: Lightroom writes
every crop key when a frame is cropped, so one that is present and
will not read is corruption, not absence, and the crop is refused
and reported rather than placed with a substitute edge or angle,
which would be a different rectangle. The placement itself is
checked too, since a finite but absurd edge with an angle still
comes out as NaN geometry; a placement that is not finite is
refused with the same report. The custom aspect ratio typed in the
window had the same hole and takes finite values only. A test feeds
the five spellings through the exposure, a curve point, and the
crop, and checks the preset and the applied edit both round-trip
through JSON; another gives every numeric key the mapper reads the
value "NaN" and reads the sidecar back.

## A sync before the rename

The edit sidecar, the XMP sidecar, the preset files, the collections
file, and the roots file were each written to a temporary file
beside the target and renamed over it, with no sync of the file
first. On a crash or a
power loss inside the filesystem's flush window, the rename can
reach the disk before the data and leave an empty file under the
sidecar's name; the export queue already synced, the others did not.
Each now writes, syncs the data, and closes the file before the
rename; the roots file, which had its own copy of the same shape
down to the part-file name, goes through the collections writer. On
a Mac the sync is F_FULLFSYNC with no fallback in the standard
library, and a volume without it, a network share in particular,
would otherwise fail every save, so a sync the volume does not
support is best effort and every other error is fatal, as SQLite
does. A failure after the temporary file exists removes it.

The cost is real: about 5 ms a file on this machine's idle disk
against 0.03 ms before, and the window's thread saves sidecars, so
a paste onto hundreds of frames now holds the window for seconds
where it held it for milliseconds. We keep the sync, since the
sidecar is the one copy of the edit, and the roadmap has the bulk
save batched (every temporary written and synced, then every rename)
or moved off the window's thread, to be measured on a network share
and a Mac first.

Other temporary-and-rename writers without a sync were listed and
left: the rejects queue, a match record, a copied look, the mask
raster cache, and the thumbnail cache, and the model, lens, and
runtime downloads, which can be fetched again. The rejects queue is
the one worth the same call; the roadmap has it.

## A missing camera profile, read and left

The review also said a missing DCP profile silently changes the
render. Partly so: the develop falls back to the file's own profile
and the Camera section says "(missing)" with a line that the file's
own colors are used, but only while that section is open; the
status line, the export's report of what the edit went without, and
the headless export's exit code say nothing, and syncing or applying
a preset that names the missing profile spreads the name without a
word. Those are roadmap lines, not a change here.
