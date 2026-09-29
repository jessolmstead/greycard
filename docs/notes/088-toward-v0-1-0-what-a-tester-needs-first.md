# 88. Toward v0.1.0: what a tester needs first (2026-09-19)

The first release is for a handful of Linux testers, and the list of
what stands between the tree and that is short and unglamorous: the
repo is private, the folder chooser fails on a fresh machine, the
filmstrip cannot reach past its first screenful, nothing builds a
downloadable, the README says `cargo run` and nothing else, and the
CLI and the editor sharpen at different points. Each was run as its
own branch in its own worktree, reviewed fresh, and fast-forwarded
onto master; this section collects them as they land. The version is
0.1.0, set once in the workspace and inherited by every crate.

**The folder chooser.** Open Folder never reached a chooser window:
`choose_folder` handed `dialog` an empty filter, and `dialog` put a
`filters` entry into the portal options unconditionally, so
xdg-desktop-portal saw a filter with an empty name and answered
`org.freedesktop.portal.Error.InvalidArgument: invalid filter: name
is empty` before drawing anything. The `filters` option is now an
`Option`, built by a small `portal_filters` helper that returns
`None` when the name is empty or no pattern survives; zvariant's
`SerializeDict` skips a `None` field, so the key is simply absent,
the same way `current_name` and `directory` are already left out
when they do not apply. `directory` was fine as it was, on `OpenFile`
against a portal reporting version 4, and `current_filter` was never
sent at all. Checked against the live portal over busctl: the old
shape reproduces the error verbatim, the new one returns a request
handle and opens the chooser, and a real `SaveFile` filter is
accepted unchanged. A unit test pins the helper.

**What the README tells a tester.** The run section said `cargo run`
and nothing else. It now says how to install a release tarball and
what `install.sh` does, what a source build needs (Rust 1.98, the
five dev packages CI installs, a network connection once for the
prebuilt ONNX Runtime), and what the program needs at run time: a
GPU for the viewport with no software renderer behind it, the
models on the same GPU with a CPU fallback that is slower but real
(`Provider::Cpu` is always last in the list and always available),
Wayland or X11, the lensfun database from the distribution's copy
if there is one and the LENS panel's fetch if not, the models
downloaded after their license is shown with their sizes named, and
where the settings, the caches and the `.gcd` sidecars live. A
reporting section says to run from a terminal since there is no log
file, to give `greycard --version` or `greycard-ui --version`, and
that a raw file that renders wrong is the most useful thing to send.
An issue form under `.github/ISSUE_TEMPLATE/` asks for the same
things. Every claim was checked against the code by a second reader;
the first draft had the lensfun fetch as automatic, which it is not,
and did not know about the system copy.

**A release is a tag.** Pushing `v<version>` runs
`.github/workflows/release.yml`, which builds `greycard-ui` and
`greycard` in release with `--locked` on ubuntu-24.04, the oldest
runner GitHub still offers now that 22.04 is in brownout, because the
glibc a binary links against is the floor of what it will run on.
`scripts/package.sh` then rolls `greycard-<version>-x86_64-linux.tar.gz`:
both binaries, the desktop entry, the icon, LICENSE, README and an
`install.sh` that copies into `~/.local` and refreshes the desktop's
caches, the procedure that had been done by hand until now, and an
`uninstall.sh` that takes it out again. The release is a pre-release
while the version is below 1.0. The one library that travels with us
is `libwebgpu_dawn.so`: the prebuilt ONNX Runtime links statically
but Dawn does not, and the build leaves a symlink to it in
`target/release` pointing into ort's download cache. It goes into
`bin/` beside the executables, copied dereferenced, because `$ORIGIN`
on their runpath is the only place they look, and both of them look,
the command line as much as the editor, since `greycard-ai` is linked
into both. That rules out a build cache in the release job, since a
restored `target` without the download cache behind it would leave
the symlink dangling; the packaging script treats a Dawn that does
not resolve as a failure rather than a tarball quietly short a
library. Three guards from the review: the tag's version must equal
the workspace's, so a mis-tag cannot label a tarball whose binaries
print another number; the tag is validated before it reaches a
shell and never interpolated into one; and the tar is deterministic,
owner 0, names sorted, mtime the commit's, gzip without its
timestamp, so a locally cut tarball is byte-identical to the
runner's and carries no builder's name. The workflow has not run
yet; the first tag is its test.

**The sharpen moved behind the lens, and the two paths agree** (an
addendum to §74). §74 recorded, while placing the defringe, that the
sharpen was not in the same place on the two paths: the CLI sharpened
inside `develop`'s `finish`, before the lens correction, and the
worker sharpened after it, so with a profile that moves pixels the
export and the viewport were different pictures. The CLI now lifts
`--sharpen` out of `DevelopSettings` before it develops and runs
`sharpen::sharpen` after `correct_lens`, with the radius the mosaic
gave and the clip level the develop returned: the worker's two
arguments, in the worker's place, on the learned-denoise path as
well. Sharpening first was wrong on its own terms and not only for
the disagreement: the correction is a cubic resample, and a resample
blurs what has just been deconvolved, while the point spread the
deconvolution assumes is the one §21 measures on the mosaic, which
the resample is no longer the scale of. `DevelopSettings::sharpen`
stays, since it is the engine's own last op for a consumer with no
geometry, but its doc now says it runs before anything a consumer
puts on top and that a consumer which resamples must leave it off
and sharpen itself. The cleaner end is to delete the field: its only
user besides the CLI was `Edit::settings()`, which the worker
overrides to `None` the moment it reads it. That waits on the UI
being free to touch. Measured on the lighthouse frame
(`4Z4A3525.CR3`, RF 50 at f/1.2, the profile carrying a `ptlens`
distortion and a 5.09 corner gain): the blend covers 23 percent of
the picture the old way and 31 the new, at the same radius 0.53 and
the same automatic threshold of 10 percent, since the corners are
lifted before the contrast is measured, so more of the frame clears
the bar. The two previews differ by 0.055 of 255 on average, 0.8
percent of pixels by more than a step, down the left and bottom
edges where the distortion moves most; the few pixels that differ by
a full step are a blown corner already ruined by the vignetting gain,
and both orders make a mess of it. One consequence the reviewer
named, parity rather than a defect: the clip level is measured
before the geometry and the sharpen now sees the corners after the
vignetting gain, which the worker has done all along. A test in the
CLI pins the order on a synthetic frame with a manual `poly3` and
the sharpen on: the result is `sharpen(lens(x))` to 1e-6 and is not
`lens(sharpen(x))`. What is still not the same on the two paths:
`--sharpen` on a picture that is not a raw does nothing at all, since
`run_develop_picture` takes no develop settings, where the editor
would sharpen it. Its own line on the list.

**A cold start, walked through.** With `HOME`, `XDG_CONFIG_HOME` and
`XDG_CACHE_HOME` at one empty directory and no lensfun on the system,
a release build does the right thing almost everywhere: `greycard
develop --preview` needs nothing fetched and takes 1.3 s on the R6
Mark II's 24 MP and 5.1 s on the GFX's 101 MP; `greycard lenses
--fetch` pulls the 430 KB database in 0.2 s and leaves the CC BY-SA
note beside it; the three denoiser tiers come down in 3.8 s and hash
as §70 recorded; `greycard-ui --snapshot` over a folder starts,
develops and quits in 3.5 s; and the whole exercise wrote nothing
outside the temp home, the ONNX Runtime being linked in rather than
fetched. Four things read badly and are now fixed. A file no decoder
reads was described in rawler's words, `No decoder found, model '',
make: '', mode: ''`, all four fields printed whether filled or not,
and now says it is not a raw this build reads, keeping the camera's
name when rawler knew one. `--ai-denoise` with a mistyped tier handed
the name to the runtime as a path and got `File at 'turbo' does not
exist` once per execution provider, after the decode; it now names
the tiers, read from the registry, before any work. `--lens` on a
machine without the database decoded and developed the frame first
and then pointed at the editor's LENS panel, which is no use at a
terminal; the database is looked up before the decode and the
message and the flag's help both name `greycard lenses --fetch`. And
`greycard lenses FILE` ended on a `{:?}` of `LensCorrection`, which
is now a sentence. What a fresh machine still cannot do is get a
model without the window: the store has no CLI route at all, so
`--ai-denoise fast` on a headless box is a dead end, and the Subject
mask's license sheet is reachable only by clicking into Masks; a
`greycard models [--fetch TIER]` beside `lenses` is the missing half.
Three editor faults are worse than they look for anyone scripting
it: a path that does not exist passes `list_files`, which checks the
extension and not the file, and opens a blank window that never says
anything; `--snapshot`, `--screenshot` and `--export` all wait on
`renderer.has_image()`, while a failed open only sets the status
text, so an undecodable file hangs the process forever with no
output and no exit code; and a failed folder chooser reaches the
user only as a line on stderr, with nothing in the window. An
X-Trans frame is refused with a `{:?}` of a 36-entry `CfaPattern`
rather than a sentence saying the mosaic is not supported yet. Each
is on the list.
