# 34. The AI features: runtime, models, and the order (2026-09-07)

Three features wait on one decision (roadmap, AI): AI denoise, AI
masks, and an eraser. What Lightroom does well with them is mostly
plumbing, and the plumbing is largely built already, so this section
records the decision and the reasons rather than a design of each.

### The runtime

The candidates, and the test each fails:

- **ONNX Runtime through the `ort` crate.** ORT is MIT, `ort` is
  Apache/MIT. Execution providers for CUDA and TensorRT, DirectML on
  Windows, CoreML on macOS, WebGPU (through Dawn) and a CPU path that
  always works. Every model below exists as ONNX or exports in a line
  of Python. RapidRAW ships its AI masks this way. Cost: a C++
  dependency; the CUDA provider wants the CUDA libraries on the
  system, which a Flatpak does not carry.
- **candle** (Hugging Face, Apache/MIT, Rust). SAM and MobileSAM exist
  as examples. No Vulkan backend; the CUDA backend needs the toolkit
  at build time; no FFT, so LaMa's Fourier convolutions would be a
  hand port.
- **burn** (Apache/MIT, Rust). Has a wgpu backend, which is the one
  path that runs wherever the app runs, and a CUDA backend. Its ONNX
  importer covers a subset of ops, and its convolutions are several
  times slower than cuDNN. Compile times are heavy. The right choice
  if we ever want zero native dependencies, and a fair choice for a
  network we design ourselves.
- **tract** (Sonos, Apache/MIT, Rust, CPU only). Reliable, and fast
  enough for a mask at 1024 px on 32 cores; a 24 MP denoise would take
  minutes.
- **Our own WGSL compute.** Tractable for one fixed small network. A
  transformer like SAM is a project.

**Decision: `ort`, loaded dynamically (`load-dynamic`), CPU provider
as the floor, CUDA/TensorRT, DirectML or CoreML when found at
startup.** The app says which it is using. burn on wgpu is the one to
revisit for the denoiser once its architecture is ours, since the
importer coverage stops mattering when the model is written in burn.

Two rules keep the repo clean:

1. **Models are data, fetched on first use.** Each lives in a cache
   directory (`~/.cache/greycard/models`) with its license text, shown
   before the download and kept beside the file. The GPL crates never
   redistribute weights, so a model's license only has to allow the
   user to download and run it. Bundling would tie the app's license
   to every model's.
2. **`greycard-core` never sees the runtime.** A `greycard-ai` crate
   owns `ort`, the registry and the downloader, and exposes a small
   trait: an image in, a mask or an image out, with the model's
   identity and version. The editor wires it in. Core keeps its
   no-model paths (the profiled denoiser, §13; brushes, §30), which is
   why those were built first.

Hardware. This machine (16 GB RTX 5070 Ti, 32 cores, 60 GB) runs every
model below in seconds and can train the denoiser. Without CUDA
libraries on Linux the default is CPU, which is fine for masks and slow
for denoise. On Windows DirectML runs on any GPU and on macOS CoreML
does, which is the same arrangement Lightroom has.

### Masks: first, and the easiest

Lightroom's Objects tool is Segment Anything. The encoder runs once per
image when the tool is picked (SAM 2 small: about half a second on a
GPU; MobileSAM: a second or two on CPU), then a decoder answers each
click or box in about ten milliseconds. That is the whole reason it
feels instant, and it falls out of keeping the embedding while the
tool is active.

The models, with licenses:

| Mask | Model | License | Note |
|---|---|---|---|
| Objects (click, box) | SAM 2 / SAM 2.1, MobileSAM, EfficientSAM | Apache-2.0 | encoder once, decoder per prompt |
| Subject, Background | BiRefNet | MIT | RMBG-2.0 is BiRefNet retrained under a non-commercial license: avoid |
| Subject (older) | ISNet/DIS, U²-Net | Apache-2.0 | fallbacks |
| Sky | SAM 3 by concept ("sky") | Meta's own license, to read | semantic models with clean weights are thin: SegFormer and Mask2Former weights are non-commercial |
| People, parts | SAM 3 by concept, or MediaPipe landmarks (Apache-2.0) driving SAM point prompts | | face-parsing nets are trained on CelebAMask-HQ, research only |
| Person detection | RT-DETR (Apache-2.0); YOLO is AGPL-3.0, which a GPL-3 app may use | | |

The plumbing already exists. The model runs at about 1024 px on the
developed preview (a downsized sRGB rendering of the base develop, no
local edits). The result comes back as low-resolution logits and is
upsampled with a guided filter against full-resolution luma into the
same 2048-wide raster the brushes use (§30), then becomes one more
`Shape` on a component. Feather, invert, add, subtract and intersect,
brushes and gradients on top, the mask overlay, and the sidecar all
work as they do for a brush. The sidecar stores the prompt (kind,
clicks or box, model id and version) and a PNG of the raster beside
it, so opening a file never needs the model and a later model can
regenerate the mask on request.

### Eraser: second, in two tiers

Tier one is a **patch layer** with a manual heal and clone, as
darktable's retouch and RawTherapee's spot removal, applied to the
working image before every other edit so the rest of the pipeline sees
the repaired pixels. Patches are stored in the sidecar cache as small
linear tiles.

Tier two fills the same layer with **LaMa** (Apache-2.0, about 200 MB,
a second or two per hole): crop a window around the hole, render it
with a fixed display-like encoding that we choose (not the user's
edit, so it inverts exactly), inpaint, invert back to linear working
space, store the patch. That covers tourists, power lines and sensor
dust. Lightroom's generative remove runs in Adobe's cloud; the open
diffusion inpainters are several gigabytes and mostly non-commercial
(SD inpainting under OpenRAIL-M, FLUX Fill dev non-commercial), so
generative fill is a later, optional tier behind the same layer.

### Denoise: the research project, and our own model

No pretrained raw-domain denoiser with a clean license exists. NAFNet
(MIT) is trained on sRGB SIDD; Restormer is academic-only; SID (MIT) is
one Sony body; the Darmstadt and RAISE sets are research-only. The
good ones (DeepPRIME, Lightroom's Denoise) all train on synthetic
Poisson-Gaussian noise added to clean raws, which works when the noise
model is calibrated. We have that model per frame (§13, §13n) and the
variance stabilizer, which is what lets one small network serve every
ISO and camera: it sees stabilized, unit-variance input.

The plan, as §13g foresaw:

- A one-to-three-million-parameter UNet (PMRID or NAFNet shaped) on
  packed four-channel Bayer at half resolution, outputting full
  resolution RGB by sub-pixel shuffle, so it replaces demosaic and
  denoise together. f16 weights.
- Trained in PyTorch on our own low-ISO raws with synthetic
  noise from our model, so the training data's license is ours.
  Exported to ONNX. Judged by the benchmark harness against the hybrid
  denoiser (§13) at high ISO, which is the acceptance test.
- A 24 MP frame should take a few seconds on this GPU and minutes on
  CPU (rough count: ~100 GFLOP per megapixel), so the output is cached
  on disk keyed by file, model and version, as a linear DNG through
  rawler's writer, which is also why Lightroom writes a DNG.
- The strength slider blends model output with the input rather than
  rerunning the model, and it keeps the noise model's per-frame ISO
  behavior.

This is days to weeks of iteration and comes last.

### The order

1. `greycard-ai`: `ort`, the registry, the downloader with license
   display, providers detected at startup.
2. Subject and Objects masks with the guided-filter refinement into
   rasters, the prompt and PNG in the sidecar.
3. The patch layer with manual heal and clone, then LaMa.
4. Sky and People, after reading SAM 3's license, else the landmark
   route.
5. The denoiser: training, benchmark, ONNX, cache.

### A first probe of `ort` (2026-09-07, parked)

Before the crate, a scratch binary against `ort` 2.0.0-rc.13 with the
`webgpu` feature, which pulls pyke's prebuilt ONNX Runtime 1.28.0 for
Linux x86_64 with the WebGPU provider (Dawn) at build time. What it
showed, so the crate starts from facts:

- The build works with no system dependencies. Dawn arrives as
  `libwebgpu_dawn.so` copied next to the binary, with no rpath, so the
  binary needs `$ORIGIN` in its rpath (or `load-dynamic`) before it
  runs from anywhere but the build tree. A packaging item.
- The prebuilt Linux variants are: none (CPU), webgpu, nvrtx, and
  cuda13+tensorrt+nvrtx. The CUDA one needs CUDA 13 and cuDNN on the
  system; this machine has neither, only the driver.
- **The WebGPU provider fails on BiRefNet_lite** at a Split node: the
  shader wants 17 storage buffers and the adapter allows 16. So WebGPU
  is not a free portable GPU path yet; each model has to be tried, and
  the fallback must be automatic. The CPU provider runs the 1024²
  model in 2.7 s on 32 cores, which is usable for a Subject mask.
- Models tried or measured, all on Hugging Face:
  `onnx-community/BiRefNet_lite-ONNX` (MIT; fp32 224 MB, fp16 114 MB;
  input `input_image` 1×3×1024×1024 f32 even for the fp16 file,
  ImageNet mean/std; output `output_image` 1×1×1024×1024 logits).
  `onnx-community/sam2.1-hiera-small-ONNX` (upstream Apache-2.0;
  `vision_encoder.onnx` + 162 MB `.onnx_data` beside it, decoder 21 MB;
  1024 input, same normalization; not yet run).
  `Xenova/slimsam-77-uniform` (Apache-2.0; 23 MB encoder, 17 MB
  decoder) is the cheap SAM for CPU.

**Resume here:** run SAM 2.1 small and SlimSAM through the probe on
WebGPU and CPU, then build `greycard-ai` with providers tried in order
(CUDA if present, WebGPU, CPU) and a model registry holding the four
files above with their hashes and licenses.

### Built: the crate, Subject and Object masks (2026-09-07)

`crates/greycard-ai` is in, and the first two masks with it. What
changed from the plan above, and what the build taught:

- **Runtime.** `ort` 2.0.0-rc.13 with the `webgpu` feature, which
  fetches pyke's ONNX Runtime 1.28 build at compile time; a `cuda`
  feature swaps in the CUDA 13 build for a machine that has the
  libraries. `runtime::open` tries the providers in order (CUDA,
  WebGPU, CPU), and a provider must load *and run* the model once to
  be kept: WebGPU accepts BiRefNet and then fails at a Split node
  (17 storage buffers, the adapter allows 16), so Subject runs on CPU
  (2.8 s at 1024²) while SAM runs on WebGPU (embed 50 ms, decode
  13 ms; 0.65 s and 60 ms on CPU). SAM 2.1's export needs graph
  optimization held at Level1: ORT's transpose optimizer throws on it
  higher. Dawn arrives as `libwebgpu_dawn.so` beside the binary, so
  `greycard-ui`'s build script adds `$ORIGIN` to the rpath and the ai
  crate's adds `$ORIGIN/..` for its tests.
- **Models** (`registry.rs`): BiRefNet lite fp16 (MIT, 115 MB, one
  file) for Subject; SAM 2.1 Hiera small (Apache-2.0, 184 MB in four
  files, the `.onnx_data` names fixed by the graphs) for Object. Each
  file carries its size and sha256; `store::fetch` downloads to a
  `.part`, checks the hash, renames, and writes `LICENSE.txt` beside
  the files. The store is `$XDG_CACHE_HOME/greycard/models` or
  `~/.cache/greycard/models`. SlimSAM (Apache, 40 MB) also ran, and
  is the fallback if a small machine wants one.
- **Refinement** (`refine.rs`): He, Sun and Tang's guided filter,
  with the preview's luma as the guide, radius a 256th of the
  preview's width, ε 1e-3; box means by running sums. Tested: a ramp
  across a luma step comes out steeper; a spike under a flat guide
  comes out spread.
- **The preview** the models see (`ui/src/ai.rs`): the base develop
  through the global look alone (no locals, geometry, vignette or
  grain) to sRGB at 2048 on the long side, by `export::render`. Made
  once per base develop; a mask keeps the preview it was made on until
  its shape changes or another file opens, so a white balance drag
  does not rerun a 3 s model. `GREYCARD_AI_DUMP=DIR` writes the
  preview, the model's mask and the refined one as PNGs.
- **In the edit** (`mask.rs`): `Shape::Subject {}` and
  `Shape::Object { picks: Vec<Pick { pos, positive }>, boxes:
  Vec<[Pos; 2]> }`, in the masks' units like everything else;
  `Shape::is_raster()` covers brushes and these, and `Mask::at_with`
  asks the caller for any raster shape. `Raster::from_data` makes a
  brush raster from a model's mask, so the shader, the finish and the
  export paths carry it exactly as they carry a brush. The sidecar
  keeps the prompts, not the raster: a Subject is found again when
  the file is opened (3 s on CPU). A PNG cache of the raster is the
  obvious next step if that grates.
- **In the editor.** Subject is a button, not a tool: it adds the
  adjustment (or the shape) and asks the worker at once. Object is a
  tool like the brush, kept in hand: a click is a pick, a right-click
  a negative pick, a drag a box, and each press re-decodes. The panel
  offers "Pick" on a chosen Object as it offers "Paint" on a brush.
  Masks are asked for from the render pass (`bake_locals` reports the
  learned shapes without a raster, `ask_for` sends them once) and
  arrive as `Outcome::Mask`; a `--screenshot` waits for them. When a
  model is not in the store the model sheet offers it with its size,
  host and license; "Download" fetches on its own thread with the
  progress on the status line, "Not now" is remembered for the
  session. Exports make any raster not yet made.

Checked on the bridge frame: Subject finds the bridge with the couple
on it, edges snapped to the railing and finials; one click on the
plaid shirt gives the shirt; the export brightens the same pixels. The
raw test set has no faces or sky to speak of, so Sky and People wait
as planned.

### Two things asked after the first look (2026-09-07)

A subject found now shows its mask overlay at once (the panel's "Show
mask" turns on when the Subject outcome arrives), so the find can be
judged before anything is adjusted through it. And the filmstrip's
pictures follow their files' turns and mirrors: the worker still makes
each thumbnail unturned, the window keeps that and shows it through
`Geometry::to_source` with the file's turns and flip (`turn_thumb`,
tested against the viewport's convention), from the sidecar for a file
not open and from the panel for the open one, re-shown whenever the
open file's turn changes, undo and redo included.
