# 90. Lesser hardware: what a MacBook Air would make of this (2026-09-19)

Everything so far has been built and timed on a 16-core Zen 5 with
AVX-512 and a 16 GB discrete card. The question was what the same
code does on a fanless laptop: an M4 MacBook Air (4 performance and 6
efficiency cores, a 10-core GPU, 16 GB shared) or the cheaper
A18-class MacBook (2 and 4, a 6-core GPU, 8 GB). No Mac has run it;
the port is a Platform item that nobody has tried. So the answer is
an extrapolation, anchored by one measurement that was cheap to take
here: the CLI develop with rayon and `taskset` pinned to fewer
threads. Times include the decode and the PNG encode, about 0.8 s
of each figure (bilinear, no CA, 32 threads: 0.81 s).

| develop                             | 32 thr | 10 thr | 6 thr  | 4 thr |
|-------------------------------------|-------:|-------:|-------:|------:|
| 24 MP R6 II, sharpen                |  1.8 s |  2.8 s |  3.0 s | 4.0 s |
| 24 MP, sharpen + profiled denoise   |  5.4 s |  9.5 s | 15.0 s |       |
| 45 MP R5 II, sharpen                |  4.0 s |  4.2 s |  6.7 s |       |

Two things to read off it. The base develop scales sub-linearly, so
losing cores costs less than the core count suggests: 6 threads are
1.7x slower than 32, not 5x, and 45 MP at 10 threads is within noise
of 32 (the wide run is memory-bound past a point, and the SMT
siblings add little). The profiled denoiser is the exception: 3x
slower from 32 to 6 threads where the rest loses under 2x, so the
non-local means has an inner loop that only pays with many threads,
or per-thread scratch whose cost does not shrink with the count.
Worth a profile on its own.

To turn the 6-thread column into a laptop: an Apple performance core
is close to a Zen 5 core in scalar code but carries 128-bit NEON
against AVX-512 here, and an efficiency core is about half a
performance core. Call it 1.5x the 6-thread column for the Air and
2.5 to 3x for the A18-class machine. So:

- **Base develop on open and on any base change**, 24 MP: 4 to 6 s
  on the Air, 8 to 12 s on the small machine, before any denoise.
  The sharpen alone, which re-runs on every sharpen slider change at
  about 600 ms here (§56), is 2 to 4 s there.
- **Profiled denoise**: 30 to 40 s per base develop on the small
  machine. Once per image if the noise settings are left alone; a
  half-minute slider otherwise.
- **The learned denoiser**: the RTX 5070 Ti is about 44 TFLOPS fp32,
  the M4 GPU about 4, the A18 Pro GPU about 2. The fast tier's 2.4 s
  (§37) is 25 s on the Air and near a minute on the small machine;
  the best tier double that. The DNG cache (§37) makes it a
  once-per-image wait, tolerable as a batch step and not as a
  slider. The CPU provider, minutes on 32 cores, is out of the
  question on 6.
- **Denoiser memory on unified memory**: the default 1024 tile peaks
  at 4.2 GB of VRAM for the best tier (§37). On a Mac that is system
  RAM. Dawn does not report running out (garbage on the AMD iGPU, a
  crash on the discrete card, §37), and on an 8 GB machine the
  outcome is a swap storm or a jetsam kill rather than either. The
  tile has to come from a memory budget, not a constant.
- **Thermals**: both machines are fanless. One develop is a burst
  and fine; a batch export of fifty 45 MP frames throttles after the
  first minute, so export throughput there is a sustained figure,
  not the burst one.

What is fine, and it is most of the interaction: tone, color, masks,
grain and white balance preview in the viewport shader per frame
(§14, §17, §31), whose cost is per screen pixel and not per image
pixel, and the base develop only re-runs when a base setting
changes. Any Apple GPU of the last five years carries that. The
code carries no x86 assumption: no `target-cpu` flag (§56 found
`native` a mixed result here anyway), rayon throughout, and the
float-to-half conversion is a hardware instruction on arm64. Memory
fits: 0.9 GB peak at 24 MP and 1.7 GB at 45 MP with the profiled
denoise (§13p), plus 190 to 360 MB for the viewport texture (§17),
so one image is comfortable on 8 GB without the learned denoiser.

What would change the picture, in the order it pays:

1. **A proxy develop for the fit view.** The Air's screen wants about
   1.6 MP of a 24 MP frame at fit. Binning each 2x2 Bayer quad to one
   RGB pixel needs no demosaic at all (no AMaZE, no VNG4, no dual
   blend) and touches a quarter of the pixels; the CA correction and
   highlights run on the binned frame. That is roughly a 10x cheaper
   base develop for the fit view, with the full develop kept for 1:1
   and export, which is what Lightroom and darktable both do. The one
   change that makes weak hardware feel like strong hardware, and the
   only one on this list that is architectural: the worker would hold
   two developed images and the viewport would pick by zoom. The
   sharpen and the denoisers cannot be judged at proxy resolution,
   which is fine, since they are judged at 1:1, where the full
   develop is the one shown.
2. **A memory budget for the learned denoiser's tile**, and the
   plausibility check kept on Metal. Cheap, and it turns a kill into
   a slow run. The existing AI item waits on a runtime that can ask
   the adapter; on a Mac the answer is the machine's RAM, which the
   process can ask the OS for.
3. **The CoreML provider with a fixed tile shape.** The tiling already
   gives a static tensor shape per tile (608x608 packed at tile
   1024), which is what CoreML wants for the Neural Engine, and the
   M4's Neural Engine outruns its GPU at fp16 convolutions by a wide
   margin. That could bring the fast tier under 10 s on an Air. A
   probe (`crates/greycard-ai/examples/probe.rs`) before the port is
   otherwise planned, since ort's CoreML provider has its own list of
   unsupported ops and falls back to CPU per node without saying so
   loudly.
4. **Sharpen and CA on the GPU.** Both are separable blurs and a
   deconvolution, the shape WGSL compute likes, and the wgpu device
   exists. The Engine item "GPU implementations of engine ops" has
   waited on the editor showing which ops must be interactive; this
   is that showing: the sharpen slider, and the CA on every base
   develop.
5. **The non-local means' scaling**, per the table.

Before any of it: build the CLI on an M-series machine and take the
table again there. The multipliers above are guesses from a very
different CPU, and the one Mac number would replace all of them.

**Addendum: an M5 MacBook Pro.** The base M5 (4 performance and 6
efficiency cores, a 10-core GPU, 16 to 32 GB) is the Air's layout
with a fan and ten to fifteen percent more per core, so it holds the
6-thread column times about 1.3: a 24 MP base develop in 3.5 to 4 s,
45 MP in 8 to 9 s, 15 to 20 s with the profiled denoise, and the fast
tier around 20 s through Dawn. An M5 Pro or Max (12 to 18 CPU cores,
mostly performance cores, 20 to 40 GPU cores, 36 GB and up) is a
different case: the table saturates between 10 and 16 threads here,
so a dozen or more Apple performance cores land a 24 MP develop in 2
to 3 s and 45 MP in about 4 s, which is this desktop; the Max's
memory bandwidth is several times this box's and the 45 MP develop
looked memory-bound past 10 threads, so the big frames may come out
ahead. The profiled denoise, which scales worst, stays behind at 7 to
9 s against 5.4. The GPU is the split: conventional fp32 is about 5
TFLOPS on the base M5 and 20 on a Max against 44 on the 5070 Ti, so
through Dawn the fast tier is 20 s and 5 s, the best tier double
each. What M5 adds is a neural accelerator in every GPU core, which
Apple quotes at over four times the M4's peak for AI, and which WGSL
compute through Dawn never touches; CoreML does. So on M5 the CoreML
provider is the whole story for the denoiser: with it a Max could
match the desktop and a base M5 could land the fast tier under 5 s.
Priorities shift with the tier: on a Pro or Max the proxy develop
matters less, since the full develop already lands in the two
seconds the 300 ms rest hides well, and CoreML moves to the top; on
the base M5 the Air's order holds, the proxy develop first.

**How the CoreML provider would go in.** The pieces, from what the
code and the ort crate already have:

- *The build.* ort's `coreml` feature (`ort-sys/coreml`) links the
  provider; pyke's macOS binaries are expected to carry it, to be
  confirmed on the first Mac build. A `coreml` feature on
  `greycard-ai` beside `webgpu` and `cuda`, on by default for
  `target_os = "macos"`. A `Provider::CoreMl` variant in `runtime.rs`,
  dispatching `ort::ep::CoreML::default()` with the ML Program format
  (the legacy NeuralNetwork format takes fewer ops and no fp16
  program), compute units `All` (CoreML then puts each op on the
  Neural Engine, the GPU or the CPU as it sees fit; `CPUAndNeuralEngine`
  is the experiment that says whether the Neural Engine alone
  carries it), `with_static_input_shapes(true)`, and
  `with_model_cache_dir` under `~/.cache/greycard/models`, since
  CoreML compiles the graph on first load and the compile is seconds.
- *The shape.* The Neural Engine wants a fixed tensor shape, and the
  tiling already gives one: every tile, edge tiles included (the
  `Mirror` fills them to size), is `side = tile + 2 * margin` mosaic
  samples, 1216 at the defaults, so `packed` is always
  [1, 4, 608, 608] and `rgb` [1, 3, 1216, 1216]. The ONNX file says
  `h` and `w` are free (`export.py`'s `dynamic_axes`), and the graph
  carries the dynamic plumbing that comes with that: three `Shape`,
  `Gather` and `Unsqueeze` and a `ConstantOfShape`, the nearest
  `Resize`'s size arithmetic. With the dimensions fixed at load,
  constant folding removes those and the whole graph is static. Two
  ways to fix them: ORT's free-dimension override on the session
  (`AddFreeDimensionOverrideByName`, if ort exposes it; one weight
  file stays), or a second export per tier with no `dynamic_axes` at
  608x608, a new registry entry each with its hash. The override
  first. One more reason the fixed shape is needed: PixelShuffle
  exports as `DepthToSpace` in CRD mode, which the CoreML provider
  documents as taking only with a fixed input shape.
- *The ops.* Seventeen `Conv`, sixteen `LeakyRelu`, five `Concat`,
  three `MaxPool`, three nearest `Resize` by 2, two `DepthToSpace`.
  All on the CoreML provider's ML Program list. What decides the
  speed is whether every node lands on one device: the provider
  partitions the graph and any node it declines runs on the CPU with
  a copy at each boundary, and a graph cut in three is slower than
  WebGPU. `with_profile_compute_plan(true)` prints where each op
  went; that print, from `crates/greycard-ai/examples/probe.rs`, is
  the first thing to read on the Mac.
- *Precision.* The Neural Engine computes in fp16. The stabilized
  space is unit-variance with values in the low hundreds at most, so
  fp16 holds it; the accumulation inside a 3x3 convolution over 384
  channels is where fp16 could show, and the probe's CPU-against-CoreML
  comparison on one tile (the §37 habit) says whether it does. The
  plausibility check stays on, since a silent wrong answer is the
  failure mode on every provider so far.
- *Which first.* `open` settles on the first provider whose warm run
  passes, not the fastest, so on macOS CoreML goes before WebGPU on
  the strength of a measurement, not a guess: if CoreML's compute
  plan has fallen back to the CPU, WebGPU wins and the order flips.
  Timing the warm run and keeping the faster is the general fix and
  a small one.
- *Memory.* Unified, so the tile budget item applies as it does for
  Dawn; on 36 GB and up it stops mattering, and a 1536 tile becomes
  affordable, which saves the 11 percent of margin the 1024 tile
  costs.

**The editor's front door.** The three editor faults §88 left on its
list are gone, with the about line as a fourth, each its own commit
in `greycard-ui`'s `main.rs`. `--version` was the command line's
alone: the editor's clap `#[command]` carried `about = "greycard
editor spike"` and no version at all, so a tester asked which build
they ran had nothing to give; it takes `version` from the workspace
now and prints `greycard-ui 0.1.0`, under an about line that says
what the program is rather than what it was called while it was
being tried out. `list_files` checked a path's extension and not the
path, so `greycard-ui /nowhere/x.CR3` opened a window that stayed
blank and never said anything; an existence check at the top of the
function names what it cannot find and the process exits one before
any window, the way an empty directory already did, and a test pins
both halves. The third was the worst for anyone scripting it:
`--snapshot`, `--screenshot` and `--export` all wait on
`renderer.has_image()`, while `Outcome::Failed` and
`Outcome::ExportFailed` only set the status line, so a file that
would not decode hung the process forever with no output and no exit
code. The state now remembers whether the command line asked for any
of the three; both outcomes print their message to stderr whatever
the run is, since the status line was its only trace even with
someone watching, and on a batch run they mark the failure and quit
the loop, `main` returning `ExitCode::FAILURE`. The review found the
same hole at the other end, where the picture develops and the file
cannot be written: the snapshot and screenshot arms printed the OS
error and quit as though they had succeeded, so `--snapshot
/no/such/dir/x.png` came back zero with nothing on disk. Both arms
mark the failure now. On a `bad.CR3` of seventy bytes of text,
`--snapshot` and `--export` end in a second or two with the
decoder's sentence and no file written; a real frame still
snapshots, still exports, still exits zero; a batch run over a folder
quits on the selected file's failure rather than moving on; and a
window opened with no such flag still stays up with the failure in
its status line, which is what interactive use wants. Last, the
folder chooser's failure at launch, which the Open folder button had
already learned to say, sets the same words on the window instead of
reaching stderr alone. The headers and the crate description went
with it: they all still called this a spike, which it has not been
for some time.
