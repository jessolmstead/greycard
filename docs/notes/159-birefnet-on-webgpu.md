# 159. BiRefNet on WebGPU (2026-09-23)

Roadmap v0.3.0's line, built in wave A by an opus author and read twice
by an opus reviewer who rebuilt the file and retook the timings.

This is §97's second fix. The Subject model now runs entirely on the
card: one run takes 0.16 s where the CPU takes 2.9 s. The graph is
rewritten once, in Python, into a file of our own, published at
`huggingface.co/jessolmstead/BiRefNet_lite-ONNX-webgpu`.

**§97 was wrong on two counts.**

- It counted 50 Splits of sixteen outputs cutting a 16 by 16 grid.
  The 50 are 33 Splits of thirty-two outputs and 17 of sixteen, and
  they cut a 32 by 32 grid.
- It said nothing else in the graph troubles the provider. The twenty
  deformable convolutions leave 420 nodes to the CPU. With only the
  Splits rewritten, a run takes 1.37 s, not a fraction of a second.

The facts follow.

**What the provider declined.** This comes from the session's verbose
log ("Node placements"). ort rc.13's default logger prints nothing
without its `tracing` feature, so `examples/subject_bench.rs` installs
its own logger when `BENCH_VERBOSE` is set. On the onnx-community
`model_fp16.onnx`:

- *Fails at run time:* the 50 wide Splits. The decoder cuts its input
  into a 32 × 32 grid of patches, then concatenates them with Concats
  of 1024, 256, 64 and 16 inputs. The provider accepts the Splits at
  partition time and fails on the first run at `/decoder/Split_33`:
  "Too many storage buffers in shader. Current: 17, Max is 16". The
  big Concats are fine, because ONNX Runtime's WebGPU Concat binds its
  inputs in batches.
- *Left to the CPU at partition time:* 420 nodes, all in the twenty
  deformable convolutions (`dec_att/aspp1` and `aspp_deforms.0–2` in
  each of five blocks). The counts are Cast 100, Clip 80, Concat 80,
  Reshape 80, Add 40, Sum 20 and Split 20, with 100 memcpys each way at
  the boundaries.
  - Each deformable convolution floors its sampling coordinates in
    fp16, casts them to int64, then Slices, Adds, Clips, Reshapes and
    Concats them as int64 before a GatherND. The provider has none of
    those kernels for int64 ("webgpu kernel not found in registries").
  - The CPU Splits are ORT's own `GatherSliceToSplitFusion` of those
    int64 Slices.
  - The fp16 four-way `Sum` that weights the four bilinear taps has no
    WebGPU kernel either. It runs on the CPU with an inserted
    fp16→fp32 cast on each input and on the output, which accounts for
    the other 80 Casts.

**The rewrite** (`tools/ai/birefnet_webgpu.py rewrite`) has three
passes:

1. Each Split that binds more than 16 storage buffers (the input plus
   more than 15 outputs) becomes one Slice per output, with the same
   axis, offsets and output names. That turns 50 Splits into 1328
   Slices.
2. Each Sum becomes a chain of Adds (20).
3. The int64 coordinate chains stay in the float type (20 chains, 80
   GatherNDs). The Cast to int64 becomes an Identity, the chain's
   integer constants become fp16, and a Cast to int64 goes in just
   before each GatherND's indices.

Every chain in the shipped file is
Floor(fp16) → Cast → Slice → [Add 1] → Clip(0, B) → Reshape → Concat → GatherND.
The Clip upper bounds B are 33, 37, 65, 69, 129, 133, 257 and 261, and
the only arithmetic is one Add of +1, forty in all. So every value is
an integer. Before the Clip, one Add can round a huge value in fp16.
Rounding is monotone and leaves representable values alone, so
Clip(round(v)) = Clip(v) for an integer v when the bounds are within
±2048. After the Clip every value is at most 261, which fp16 holds
exactly.

**Pass 3's guard.** A review found that the first version's guard took
chains it could not prove. It checked only that constants and Clip
bounds stayed within 2048. The review's adversarial graphs, run
through the real function and ORT, broke it:

- Floor → Cast → Add 1000 → Add 1000 → Sub 1500 → Clip(0, 2000) at
  x = 1001 gives 1500 in fp16, where the original gives 1501.
- Clip(0, 2048) → Add 1 at 2048 gives 2048, not 2049.
- With no Clip at all, Add 1 at 2048 fails the same way.
- With no Floor, Cast truncates: Add 1 → Clip at −0.5 gives 0, not 1.
- A Concat with an int64 input from off the chain passes
  `check_model`, and then ORT refuses to load it.
- Unnamed nodes break the first version two ways. The review's own
  graph lost its topological order, because the inserted Casts were
  keyed by the GatherND's (empty) name. The selftest's unnamed case
  fails with "initializer name is not unique", because the converted
  constants were named after their (empty) node names.

None of these touched the shipped file, whose chains all have the
shape above. The guard is now a proof over the chain:

- the Cast's input comes from a Floor;
- an interval is carried through the chain in graph order;
- every constant is an integer of at most 2048;
- an Add or Sub whose result may leave ±2048 makes the value
  "rounded", and only a Clip with bounds within ±2048 may take a
  rounded value;
- every index that reaches a GatherND is exact;
- every Concat input is on the chain;
- the inserted Casts are keyed by node identity.

`selftest` holds eleven cases. Seven must be refused: the review's
cases above, plus a Mul on the chain and an Add of 3000. Four must be
taken: BiRefNet's shape, with and without the Add; an Add after the
Clip within range; and unnamed nodes. Each case that moves is checked against the
original on ORT and against true fp16 arithmetic in numpy. The first
version's pass fails six of the eleven. `check` runs the selftest
first. The rewrite still gives the same bytes and hash as before.

After the rewrite the verbose log says "All nodes placed on
[WebGpuExecutionProvider]. Number of nodes: 5595". No node runs on the
CPU, and no memcpy remains except the input and output.

The output is byte-identical from run to run. With onnx 1.23.0 and
protobuf 7.36.2 (`tools/ai/requirements.txt`), input sha256 `d39b89…c402`
gives 113 778 088 bytes, sha256
`0a019d6ba73c9cedc9a251f8c9390b196ff6399acd281a2872692861abbd78c2`.
A reviewer rebuilt it in a fresh venv to the same hash.

**The numbers.** The frame is 066A3439.CR3, an EOS R5 frame at
8352 × 5586 (45 MP), developed by the CLI and scaled to the editor's
2048-pixel preview. The model sees 1024², so the frame's size stops
mattering once the preview is made. Each case is one warm session,
built and run once, then five timed runs of the whole `mask` call
(planes, run, sigmoid). The machine is a Ryzen 9 9950X3D and an RTX
5070 Ti (driver 615.71.09, Vulkan through Dawn), with ORT 1.28 via ort
rc.13, in a release build. To reproduce: `sh target/work/timings.sh`.

| model, provider | session | first run | warm median (min) |
|---|---|---|---|
| original, CPU | 1.17 s | 2.97 s | 2.87 s (2.80) |
| original, WebGPU, wide Splits forced to CPU | 1.43 s | 1.90 s | 1.57 s (1.52) |
| Splits alone rewritten, WebGPU | 1.65 s | 1.61 s | 1.37 s (1.34) |
| **full rewrite, WebGPU** | 1.66 s | 0.28 s | **0.164 s (0.163)** |
| full rewrite, CPU | 1.36 s | 2.78 s | 2.72 s (2.69) |

The two reviews reproduced 0.164 to 0.165 s on WebGPU against 2.77 to
2.89 s on the CPU. The ignored Rust test in a debug build reports
0.267 s a warm run on the disc. A guess, not measured: the
unoptimized planes and sigmoid account for the gap.

- *The original with the fallbacks.* The original cannot run on WebGPU
  as it stands; it fails. The "with the fallbacks" row forces its 50
  wide Splits onto the CPU with ONNX Runtime's `forceCpuNodeNames`.
  That puts 470 nodes on the CPU and 1380 `MemcpyFromHost` on the
  card, and it is barely faster than the CPU alone.
- *The Splits alone.* A profile (`BENCH_PROFILE`) of the Splits-only
  variant spends 0.55 s in `MemcpyToHost`, 0.21 s in `MemcpyFromHost`,
  0.29 s in the CPU Sums and 0.24 s in the CPU casts around them.
  Passes 2 and 3 remove that CPU island.
- *The full rewrite.* Profiles of the full rewrite put 105 to 112 ms
  in kernels of a 187 to 201 ms profiled run. The two largest shares
  are close, and their order changes from run to run:
  - the author's: Transpose 24 ms, then the 1588 Slices 20 ms;
  - the reviewer's: Slices 22 ms, then Transpose 16 to 19 ms.

  The 16 remaining Splits come next, at 15.5 to 16 ms. Of the Slices,
  1328 are the Split rewrite's. Rewriting the patch grid as a Reshape
  and Transpose would save part of the Slices' share, which is not
  worth it now.

**ort's WebGPU options are all ignored.** Every `ep::WebGPU` option in
ort rc.13 is ignored, not just `with_force_cpu_node_names`. The crate
sets its keys with the `ep.webgpuexecutionprovider.` prefix and passes
them through `SessionOptionsAppendExecutionProvider("WebGPU", …)`,
which prefixes them again. The session's options dump shows
`ep.webgpuexecutionprovider.ep.webgpuexecutionprovider.forceCpuNodeNames`,
and ONNX Runtime ignores it. The reviewer's probe showed
`with_device_id(99)` silently ignored the same way. The workaround is a session config entry with the single-prefix
key (`ep.webgpuexecutionprovider.forceCpuNodeNames`, one name a line).
This is worth an upstream issue:

> `ep::WebGPU`'s options never reach ONNX Runtime: every builder
> method sets `ep.webgpuexecutionprovider.<key>`, and
> `SessionOptionsAppendExecutionProvider("WebGPU", …)` adds the prefix
> again, so the session sees
> `ep.webgpuexecutionprovider.ep.webgpuexecutionprovider.<key>`
> (e.g. `…forceCpuNodeNames`, `…deviceId`) and ignores it.
> `with_device_id(99)` is silently ignored. Setting the key with
> `SessionBuilder::with_config_entry` works.

**CPU against WebGPU.**

- *The CPU equality is by construction.* The rewrite gives the same
  matte as the original on the CPU (max 0, in Python ORT 1.30 and in
  the Rust side's 1.28). That does not test fp16 rounding: ORT's CPU
  provider runs consecutive fp16 Adds in fp32 (2048 + 1 + 1 gives 2050
  on the CPU and 2048 in true fp16).
- *What the rewrite adds on the card.* On the card, the Splits-only
  variant against the full rewrite differs by max 0.065 and mean
  2.5e-5, with 15 pixels changing sides of one half, all on the
  contour. That is the Sum-to-Adds pass rounding each partial sum in
  fp16 where the CPU Sum did not.
- *Against the CPU.* For the raw 1024² matte, taking the original on
  the CPU as reference, the rewrite on WebGPU differs by max 0.311 and
  mean 4.3e-4. 1.17% of pixels move by more than 1/255, and 332 pixels
  (0.03%) change sides of one half.
  - The flips all sit within 1 px of the reference's half contour.
  - Of the pixels moved by more than 1/255, 92% are within 2 px of it
    and some are up to 9 px away. So it is an edge effect, but a
    wider band than two pixels.
  - The original on WebGPU with the fallbacks gives nearly the same
    figures (max 0.307, mean 4.3e-4, 1.18%). Most of the difference is
    the card computing the fp16 graph in fp16.
- *After refinement.* After the editor's guided refinement, comparing
  its cached 3071 × 2048 rasters for the same Subject shape
  (123A6932.CR3, another 45 MP R5 frame), the two differ by max 0.051
  and mean 6.7e-5, and 0.004% of pixels change sides.

Which of the two is closer to the fp32 model was not checked.

**In the editor.** The frame is 123A6932.CR3 with its sidecar's
Subject adjustment, run through `--show-mask 1 --screenshot` in a
headless mutter, with the mask cache emptied before each run. The
time from "developed" to the screenshot:

- The original, first launch, where WebGPU is tried and fails: 10.5 s.
- The original on later launches, where the failure is remembered:
  4.5 to 5.4 s.
- The rewrite: 2.6 to 2.9 s at load average 10 to 15, of which about
  2 s is the WebGPU session build and its warm-up run. No run was on a
  quiet machine.

The first mask of a session is now mostly the session build. Every
later mask costs the 0.16 s.

**The registry.** The original entry stays unchanged. The new
`SUBJECT_WEBGPU` entry has:

- id `birefnet-lite-2024-fp16-webgpu` and one file,
  `model_fp16_webgpu.onnx`, of 113 778 088 bytes with the sha256
  above;
- URL
  `https://huggingface.co/jessolmstead/BiRefNet_lite-ONNX-webgpu/resolve/main/model_fp16_webgpu.onnx`;
- license MIT, with attribution to Zheng Peng et al. and the
  onnx-community export.

`Model` gained `modified: Option<Modified>`, which holds what changed
and the upstream notice. For such a model, the `LICENSE.txt` the store
writes says greycard publishes a modified copy and what changed. It
then carries BiRefNet's MIT text in full: "Copyright (c) 2024
ZhengPeng" and the permission paragraph, since MIT wants both with
every copy, not a link. The model sheet's note says greycard
publishes the copy, in place of "greycard does not ship this model".

The file went live on 2026-09-23. `greycard models --fetch
birefnet-lite-2024-fp16-webgpu` into an empty store fetched it in
7.6 s and checked the hash, and the LICENSE.txt carries the full
notice. `--fetch all` into an empty store brought down all seven
models.

**Which file.** `subject::model_for(store, providers, unavailable)`
decides, in this order:

1. The rewrite, if WebGPU is on offer and the store has it.
2. Whichever of the two the store has, the original first. Either
   file works with either provider list.
3. With neither in the store: the rewrite where WebGPU is on offer,
   unless its id is unavailable this session (declined, or its fetch
   failed); otherwise the original.

The editor passes its declined and failed lists through `ask_for`'s
new `step`. So a WebGPU machine whose fetch of the rewrite fails
(offline, say) says "the GPU Subject model could not be fetched …; the
original is offered instead" and offers the original. Declining the
rewrite does the same, and declining the original too leaves the
shape waiting. A model whose fetch failed is not offered again that
session, as one declined is not; offline, the sheet would otherwise
come straight back. A later successful fetch clears the failure.

A user who already has the original keeps it until they fetch the
rewrite by id or remove the original.

The mask cache's key uses the Subject model actually loaded, so a
rewrite arriving mid-session does not file the original's matte under
its id.

**`--fetch all`.** It no longer stops at the first model that fails.
Each failure is printed and passed over, and the command exits
non-zero at the end naming every failed id.

**Tests.**

- *`tools/ai/birefnet_webgpu.py`:*
  - `selftest`, the eleven cases above;
  - `check`, which runs the selftest, then fails on a Split past the
    bound or a Sum or coordinate chain left.
- *greycard-ai:*
  - `the_webgpu_rewrite_answers_as_the_original` (ignored; needs
    `GREYCARD_MODELS`, optionally takes `GREYCARD_SUBJECT_PICTURE`);
  - four tests of the choice between the files;
  - the registry test checks that a modified model carries a
    copyright notice and that the note holds the MIT text.
- *greycard-ui:*
  - `a_subject_turns_to_the_original_when_the_gpu_model_cannot_be_had`
    walks the offer through the faked failures. The rewrite fails, the
    original is offered, the original fails, and the shape waits with
    no third offer. After that come a decline and the original's
    arrival;
  - `the_offer_says_who_publishes_the_model`.
- *greycard-cli:* `a_failed_fetch_does_not_stop_the_rest` fetches two
  registry entries on an unreachable URL and gets both failures back.

The seven ignored greycard-ai model tests pass with the store set in a
debug build: 37 s at load 10, and 43.8 s under heavier load in the
review.

The examples `probe` and `subject_bench` now need the `webgpu`
feature, so a build without it no longer fails on them.

**What is left.**

- *Mac and Windows adapters.* The remaining Splits need 9 storage
  buffers. On an adapter that allows fewer, the rewrite would fail at
  the warm-up and fall back to the CPU. `check --bound 8` shows what
  would still need rewriting.
- *The first-mask cost* is now the WebGPU session build.
- *The ort issue* above, to file.
- *A way in the editor* for a user with only the original to fetch
  the rewrite.

**The review.** The file held from the first pass: the reviewer
rebuilt it in a fresh venv to the same hash and got the same 0.164 s.
What was wrong was around it. The branch as first written preferred a
file nobody could fetch: `--fetch all` stopped at its 404 before SAM,
LaMa or a denoiser, and a WebGPU machine with an empty store was
offered the rewrite, got the 404, and was offered it again, with a
decline leaving the Subject shape masking nothing and no path to the
original. The pass 3 guard took chains it could not prove, which the
reviewer showed with five adversarial graphs; the "max 0 on the CPU"
claim was true by construction and not evidence for the fp16
argument; the LICENSE.txt the store wrote carried a link and not the
notice MIT requires; the offer text said greycard does not ship the
model; and the draft stated the new facts without saying §97 was
wrong. The second pass reran the guard's eleven cases, fetched all
seven models into an empty store from the live URL, read the written
notice against upstream's, and merged the branch against master in a
scratch tree to check for semantic conflicts. A third small round
stopped an offline machine being re-offered a model whose fetch had
just failed.
