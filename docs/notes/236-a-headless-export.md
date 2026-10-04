# 236. A headless `--export` (2026-10-03)

Roadmap lines: "A headless `--export`: the export is a worker job
already, the window only lends it the wgpu device and reads the edit
and the sheet ..." and, from the bugs, "A `--export` run can stall on
the desktop ... make the export headless and it cannot happen." Built
by an opus author and read three times by an opus reviewer.

**The export never needed the device.** The item assumed the headless
run would make its own wgpu device, as the worker's tests do, and hand
it over the way the window did. It does not, because nothing in an
export would use it. Since §18 the export is the CPU reference:
`open_picture` and `export_other` call `develop_job` with no GPU
context, a base whose CA correction ran on the GPU is not reused by a
develop without one (the test
`an_export_after_a_gpu_base_develops_the_reference_s` pins that), and
a develop whose sharpen ran on the GPU leaves no CPU picture behind,
so the export makes its own. The finish (`export::render`), the mark
and the encoders are CPU code. The learned denoiser and the learned
masks run on ONNX Runtime's providers, which choose their own device
and never touched the window's. What the window lent the worker served
the viewport's develop, and an export run's viewport was a picture
nobody looked at.

So the headless run asks for no device. A device would cost the run
its startup (the adapter, the device, the context's pipelines) and add
a way to fail, no adapter, for a run that has no use for one. On a
machine with no adapter (checked with `VK_DRIVER_FILES` and
`VK_ICD_FILENAMES` pointing at nothing and `WGPU_BACKEND=vulkan`, no
display) the export runs and writes the same pixels as the windowed
one on the NVIDIA desktop: nothing to report, which is the point. The
software fallback in the pool is still the window's question and not
this one's.

**One path, not two.** `--export` is headless whatever else the
command line says; the windowed export is gone, not kept beside it.
What the window did for it (`export_then_quit` in the develop's
delivery, the quit on `Exported` and `ExportSkipped`, the set's quit
on `SetDone`) is taken out, so there is no second path to drift.
`headless.rs` makes the run from the functions the window made it
from: the sheet is `opening_sheet` (the remembered sheet, or
`--export-preset`'s, with `--long-edge` and `--on-exists` over it),
the single file's format is `batch_settings`, a set's names are
`queue::names`, a `--preset` is `preset_at_start`, the overrides are
`Overrides` (now built in one place, `Overrides::of`, for both), a
sidecar is read by `rows::from_disk`, and the export is noted by the
same `record_export_on_disk` the window uses for a frame it has let go
of. It hands the worker one `ExportSet`, `--export FILE` as a set of
one whose name is the path given, and waits on the set's outcome; the
worker writes every frame of it as it wrote a set frame that was not
the open file. The run's last line is said on stderr whichever way it
went (the terminal shows the log from warnings up, and that line is
the one a script wants). The wait polls whether the worker's thread is
still there, so a panic outside any job's guard ends the run rather
than hanging it.

**Three exit codes.** 0 is every frame written as its edit asks, or
left where `--on-exists skip` says to leave it. 1 is a frame not
written, or a run that could not begin (a flag that names something
the run cannot do, a frame that will not decode, a worker gone before
the set was done), with the reason. 2 is every frame written, but some
without something their edit names that this machine has not got or
could not run: a look not in the look directory, a learned mask
(Subject, Background, Sky, Object), a fill, or the learned denoiser
whose model is not downloaded or would not run. Each is named on
stderr with its frame, and the last line counts the frames. A missing
look was a warning and exit 0 at first; it is the same loss as a
missing model (the picture differs from the one the edit describes),
and a parity script has to be able to trust 0, so it counts the same.
A look at no strength changes nothing and is not counted. The window
has always drawn such a frame without the shape and offered the
download; a script has nobody to offer it to, and a file that silently
lacks a subject's lift (a Subject frame with an empty model cache came
out with 14.5 percent of its pixels different) is not a success to
report as one. It is not a failure either: the file is there, and is
what the window would have shown. So a code of its own, which a caller
can treat as either. The worker gathers these where the export is
made: the develop's report (`left_out_of`, from the outcome the window
reads for its own status line, kept on `Last` for the open frame's
reuse), and the look and the masks `finish_export` could not have.
They ride on `Done::Exported` and `Outcome::Exported`, so the window
logs them too and its status line says how many. Two things keep the
count honest. A develop that keeps its base reports the denoiser
`Kept`, which said nothing even when that base was the engine's
stand-in for a missing model, so a base now carries the report it was
made under (`Base::stand_in`) and `left_out_of` reads it through a
kept one. And `finish_export` asks for no mask of an adjustment
switched off or with nothing live in its mask, as `finish::Local::of`
never draws one: a Subject shape in an adjustment switched off gave
exit 2 over a picture identical to the window's, and ran the model for
nothing when there was one.

**Leaving by `_exit`.** A run whose edit loads a model loads ONNX
Runtime's WebGPU provider, which is Dawn, and Dawn's Vulkan instance
is torn down by a static destructor that dies inside libvulkan as the
process exits: SIGSEGV, exit 139, after the file is written and every
line logged. The window has always ended that way after such a
session; nobody read its exit code. A batch caller does, so the
headless run leaves through `libc::_exit` with its code once the
worker is stopped and joined and stdout is flushed (the log's file and
stderr are unbuffered). Skipping the static destructors loses nothing:
every file the run wrote is closed. Windows returns from `main` as
before; the crash has not been seen there, and has not been checked.
The teardown itself is Dawn's, or the order in which the process
unloads it and libvulkan, and is not ours to fix here.

What the command line overrides, each covered: `--exposure`,
`--develop-temperature` and `--agx` on the frame opened, never onto
its sidecar; `--preset` onto the frame opened's sidecar as a history
step, written where sidecars go; `--export-preset`, `--long-edge` and
`--on-exists` on the sheet, the path's extension over the sheet's
format for one file; `--also` for a set into a folder, with the
window's refusals (`check_also`) unchanged; `--no-sidecars`,
`--sidecar-folder` for what is read and where the export's record is
written; `--xmp-sidecars` (or the Settings sheet's choice) for the
`.xmp` written with it, as every sidecar write of the window's writes
one (`write_sidecar`: the meta, and the camera's tag composed with the
frame's turns). The sidecar the note is written into is read as an
open reads it (`load_sidecar_said`), the `.xmp` adopted where another
tool changed it since: read with a plain `Sidecar::load`, the note's
write put the `.gcd`'s old rating and turn back over the other tool's,
on this path and on the window's for a frame it had let go of. With a
folder and no `--also`, the frame opened is the one the settings
remember in it, else the first, as the window opened it.

Refused, with the reason, before a frame is begun: `--snapshot` and
`--screenshot`, which capture a window the run has not got; `--filter`,
`--roots` and `--all-roots`, which would change which frames the rows
name and need the library's index to; `--library`, an index the run
neither reads nor writes; `--turn` and `--preview-temperature`, a key
pressed and a slider set on the window for a capture, neither of which
reached the windowed export either (`--develop-temperature` is the
temperature an export takes); `--import`, another run. §167 described
`--export DIR --snapshot` as a developer's pair, the snapshot quitting
the run once the set had begun; that pair is gone with the window's
export, and a capture of the export sheet is still `--sheet export`.
The flags that only arrange the window (`--zoom`, `--tab`,
`--display-profile` and the rest) change nothing an export writes and
are passed over.

**Where the run differs from the window, on purpose.** Each frame's
edit is its sidecar's, not the panel's round trip of it; the two are
the same edit. A sidecar from an older build is brought up to date
from the file's shape (`files::migrate_from_file`) for every frame of
a set, where the window did it only for the frame it opened. The same
step now runs where an export is noted on a sidecar read from disk
(`record_export_on_disk`): it read the sidecar again unmigrated, so a
version 3 Original crop compared its version 4 export with its version
3 self, never matched, and the log blamed the command line. That was
the window's path for a frame it had let go of as well, and is fixed
for both. `--also` is a set, so a row named twice or the frame opened
named again is one frame, where the window's Ctrl+click would have
toggled it. The library index is not told of the sidecar written: its
next pass over the folder finds it by its stamp. Nothing else is
written: no settings, no thumbnail cache, no index, no export queue.

**Learned masks: the export is the reference, the window's is not.**
The pixels match the window's whenever no learned mask is in the edit.
With one they need not, and the run with no window is the side that is
right. The export's picture is the CPU reference develop, and a mask
the export makes is made from that develop's base. The window's Export
button finds the mask its viewport already asked for, kept in memory
by the adjustment and the shape (`Ai::raster`'s cache; a fill
likewise, `fill_kept`), and that one was made from the viewport's
develop, whose CA ran on the GPU. Measured by the review on two
frames: AE 174 on a Subject frame and 2.7 on a Sky frame between
master's windowed export and the headless one, and AE 0 between master
run with `--cpu-ops` and the headless one, which puts the whole
difference in which base the model looked at. The disk cache of made
masks (`masks/*.png` beside the models) has the same gap from the
other side: it is keyed by the file, the model and the shape, not by
the develop, so a mask made once is served for any later edit of the
frame, a changed exposure or white balance included, and whichever run
made it first decides what every later one draws, window or not. We
leave the caching as it is on this change; the pool takes an item to
key both caches by the base's develop, or to have an export make its
own masks from its own base, so the window's export and the command
line's agree with each other and with the reference.

**Same pixels.** In the crate,
`an_export_with_no_window_writes_the_window_s_pixels` writes a linear
DNG of a textured ramp (a raw the engine opens, made in the test, so
no sample raw goes in the repository), gives it a sidecar with an
exposure and tone of its own, and plans the run with `--agx`,
`--exposure`, `--develop-temperature` and `--long-edge`. The window's
half is the jobs it sent its worker, in order: a device of the test's
own lent (high performance, as the window's), the frame opened and
developed on it (the sharpen ran on the GPU), the open's blend taken
into the edit, then `Job::Export` of the open frame. The run with no
window writes the same frame. The two files are pixel-identical and
byte-identical but for the moment each was written, and the test says
so. The integration test `headless_export` runs the binary with
`DISPLAY` and `WAYLAND_DISPLAY` taken away: a frame written and exit
0, a frame that will not decode exit 1 with the reason on stderr, and
`--screenshot` refused.

By hand, against master's release build (c45945c) run windowed under a
headless weston, on raws from four makers and a DNG, every case
pixel-identical (ImageMagick `compare -metric AE`: 0):
`--no-sidecars --no-display-profile --long-edge 1600` with and without
`--agx` (compare-transforms' two runs); `--agx --exposure 0.7
--develop-temperature 4800`; `--preset "Muted Slide"`; an export
preset with Display P3, High output sharpening and a text watermark;
a full-size 16-bit TIFF in Rec.2020 from the remembered sheet; a PNG
under `--on-exists increment`; a set of three makers into a folder with
`--also`; and a frame with its own sidecar (a preset step, a level, a
turn, an exposure) with sidecars on, whose `.gcd` after the run is the
window's but for the export's time and path. The bytes that differ are
the EXIF `DateTime`, the XMP `ModifyDate` and the embedded profile's
header date, all three the moment of writing; in a PNG they sit in
compressed chunks, so more bytes move, and decompressed the
differences are those dates alone. The profile's date is lcms2's
stamp when the process first builds it; a fixed one would make two
runs' files byte-identical, and nothing asks for that yet.

Wall time of the whole process, one frame at a long edge of 1600,
release build, five rounds interleaved: a 24 MP CR3 2.7 to 3.6 s
windowed against 1.2 to 1.6 s headless; a 40 MP RAF 8.1 to 8.6 s
against 4.8 to 5.0 s. The window cost its startup and a viewport
develop on the GPU that the export then did again on the CPU. A
full-size TIFF of a DNG went from 8.9 s to 2.6 s; a three-frame set
from 13.7 to 8.3 s.

**The stall.** The stall the bug line describes needed a window
waiting on the compositor. A run with no window cannot wait on one,
and the runs here went with no display at all. The batch tools
(`compare-transforms.py`, `camera-match/fit.py`,
`reference/render.sh`) pass nothing the headless run refuses, and no
longer need a headless weston around them.