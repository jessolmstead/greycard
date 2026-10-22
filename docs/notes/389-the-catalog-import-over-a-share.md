# The catalog import over a share (2026-10-21)

A tester's decade of catalog, on a network share, wrote all night
with no word from the command. Another's Mac crashed 12.5 hours into
a write: SIGBUS, "FS pagein error", two seconds after waking, in a
copy out of a raw's memory map (issue #33).

**What a frame cost.** Each frame's write read its raw whole three
times or more: `probe_path` for the starting edit, `stance_path` for
an old sidecar's migration, and again for the turn and the crop of
the image and each virtual copy. rawler's `RawSource::new` maps and
populates the file, so each read was every byte: 135 MB over the
wire for a 45 MP CR3 whose probe wants 1.5.

**One read.** `decode::probe_and_stance_counted` reads a raw once
for both: one decoder, the metadata, then rawler's dummy image for
the size and mosaic. A CR3's from its ends, as `probe_ends_path`
reads them (`CMP1`, `IAD1`, and the tail's CTMD); a RAF's size sits
past the probe's head and the TIFF formats are read whole anyway, so
those are read whole once. `FrameRead` carries the stance to
`crop_onto` and `turn_onto`. The sample set agrees with the two calls
apart on all 77 raws; CR3s read 6.3% of their bytes.

**Never mapped.** A mapped file whose volume goes away faults on its
next page, and no Rust code catches a SIGBUS. Every read the import
makes goes into a buffer: the ends, or `read_whole`. So do
`probe_ends_path`'s fallback (the camera match's survey) and
`Known::of`, which probes one file per camera and lens name for the
smart collections and still mapped after the first pass of this
work. Every mapping in the backend goes through one `mapped`, which
counts the mappings under a folder a test watches on every thread;
a test asserts a whole write maps none. A file whose name is not a
raw's (clips, layered files) is not read at all: reading a clip of
gigabytes into memory to learn it is no raw would be the cost of not
mapping it. Still mapped, by choice: a develop, `probe_path`,
`stance_path`, `white_path`, `mosaic_path`, and the indexer's
`probe_and_mosaic_path`. The indexer walks shares too and is next.

**A few at once.** Sidecars are written on four scoped threads
(`WRITE_THREADS`; `GREYCARD_IMPORT_THREADS` for measuring); results
come back in catalog order, `Writing { done, of }` counts as they
finish, and the stop is still checked once before the first sidecar.
Two catalog files that are one file here (two roots on one folder,
names apart only in case) are grouped and written in turn, or the
last rename won and the other's versions dangled. On our own SMB
share four reads at once were slower than one (52–55 ms a frame
against 40–42), so one thread reads a raw at a time behind a lock
and the four overlap the sidecar writes.

**Measured.** A turned and cropped 45 MB CR3: 135 MB read a frame
before, 1.6 MB after; 200 ms a frame on the share before, 71 after.
A thousand synthetic frames on tmpfs: 1.5 s before, 0.54 s after.
Sidecar writes on a share are unmeasured, since we write nowhere on
ours.

**On the command line and in the log.** `greycard library
import-lightroom` says its progress on stderr in the sheet's words,
now the library's (`Progress`'s `Display`), at most a line a second
through a step, always its first and last; with `-v` the log's lines
stand in. stdout keeps the report. The import logs its start, the
same progress, and what the read and the write did; a warning per
frame not written carries its error and not its path, since testers
paste logs into issues. The window's log held the library crate at
warn, so none of this reached greycard-ui.log before:
`greycard_library::lightroom_catalog` now logs at info.

**Left.** The dry run's file search is still serial (an `exists`
and a sidecar read a frame), and the collections hash each member
after the write. Raws rawler decodes but `RAW_EXTENSIONS` does not
list (PEF, NRW, SRW, CRW, 3FR, IIQ) no longer get a stance in the
import, since the library does not index them either.
