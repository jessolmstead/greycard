# §394. The index pass and the thumbnail pool map nothing

*2026-10-22*

§389 took the catalog import off rawler's memory map, because a mapped
file on a share that drops while a Mac sleeps is a SIGBUS no code can
catch (issue #33). The index pass and the thumbnail pool still mapped:
the probe and mosaic of every new or changed file, the mosaic of rows
from before the mosaic column, the orientation under an XMP turn, the
camera preview behind thumbnails, local previews, and the loupe, and
the quick develop of a raw with no preview. rawler's map is populated,
so each of those pulled the whole file as well.

## What reads what now

| Read | Now |
|---|---|
| Content hash | `std::fs`, the first 64 KB, as before |
| Probe and mosaic | CR3 from its ends; every other format read whole; a DNG's mosaic from its tags on that read |
| Mosaic alone | the same, with the same fallback |
| Orientation under an XMP turn | `probe_ends_path`, the same EXIF tag the stance read |
| Camera preview | CR3: head, 1 MiB tail, and the preview sample located with rawler's own BMFF parser; RAF: head through the embedded JPEG from the header's offset and length; others whole |
| Quick develop, the reduced develop of an edited frame, the masks | `decode_path_unmapped`: the file read whole, decoded from the slice |
| The lightbox's pair comparison, the Sky's size | `stance_path_unmapped` |

Ends that give no answer, an error, or a panic fall back to a whole
read under the guard that names a half-copied file. Only a real
preview is accepted from the ends; a thumbnail is not, so the whole
read is taken instead. Non-raw names are refused before any read.
`RawSource::new` is not reached from a pass over a folder. The
editor's own open of a frame (the develop, the loupe's 1:1) still maps,
as do single-frame reads on a user's action.

Measured on the 78 sample raws, every answer matched the whole-file
read, the preview pixel for pixel. A 45 MB CR3's index pass went from
45 MB read to about 4.5 MB (hash, probe, mosaic, maker tags), its
probe from 5–52 ms to 1–3 ms; a CR3 preview reads 17.5 percent of the
file, a RAF preview 2.5 percent. The TIFF formats (DNG, NEF, ARW, ORF,
RW2) read whole, as the populated map did, so nothing reads more than
before.

Tests count maps under the folder a pass walks (`watch_maps_under`,
`maps_under`) and assert none: the core reads, the library's pass with
an XMP turn and an old row, the UI's indexer thread with the thumbnail
and the camera picture, an edited frame through the pool, and a pair
in the lightbox. Each was shown to count one map with its read put
back. The CR3 and RAF ends paths are covered by the ignored samples
test only, since no raw fixture is committed.

## Not done

- A RAF's probe and mosaic read the whole file; the mosaic needs the
  RAF's directory past the JPEG.
- The TIFF formats read whole for the probe and the preview; a head
  would mean walking their directories, and §389 found zero gaps in a
  copy make rawler abort.
- Under an XMP turn the orientation is a second read beside the probe,
  about 1.5 MB for a CR3.
- A Sky edit reads its file twice through the pool, the stance and
  the decode.
