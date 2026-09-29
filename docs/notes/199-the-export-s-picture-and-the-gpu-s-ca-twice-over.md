# 199. The export's picture and the GPU's CA, twice over (2026-09-28)

§167 measured a frame exported from the screen against the same frame
exported as a non-open frame of a set and found them 45 pixels apart at
2048, while the set matched a `--cpu-ops` single export exactly, and
named the cause as the single export's reuse of the last develop with
the GPU's CA correction in it. The base cache already refused a GPU-CA
base for a develop without a GPU (`ca_on_gpu`); the last develop kept
for an export had no such mark.

The first fix gave it one: the last develop is a `Last` with the edit,
the turn, the picture and whether the base it came from had its CA on
the GPU, filled from the base at the moment the develop lands, and
`open_picture` hands it to an export only when that was the CPU. A
unit test builds such a cache entry with a plainly wrong picture and
shows the export develops afresh, matches a reference develop from
nothing, and then reuses that picture.

The real check found the guard was not the path this frame took. With
the fast learned denoiser on and the sharpen off, the on-screen
develop's picture stays on the GPU as a texture, so the last develop
was never filled; the export did make a fresh base, and that base came
from the learned denoiser's kept pair (`LearnedBase`), whose CA had run
on the GPU when the pair was made, and `learned_base` marked the new
base as the CPU's. The CA is applied to the mosaic before the network
sees it, so it is in both pictures of the pair and in every base
blended from them. The pair now carries `ca_on_gpu` too, set by the
develop that made it, and `pair_serves` refuses a GPU-CA pair to a
develop without a GPU, which makes the pair again on the CPU; a base
built from a kept pair takes the mark from it.

**Checked.** 5M0A1023.CR3, the fast tier on at strength 1.0, sharpen
off, full-size JPEG exports from release builds with all four XDG
directories redirected: the single export from a GPU session against
the same frame as the non-open frame of a set, against a `--cpu-ops`
single export, and the set against `--cpu-ops`.

| | before | after |
|---|---|---|
| single (GPU) vs set | 103 px | 0 |
| single (GPU) vs `--cpu-ops` | 103 px | 0 |
| set vs `--cpu-ops` | 0 | 0 |

The first fix alone left every number where it was; the log for the
single export read "base made, denoiser kept, 0.03 s" for the export's
develop, and after the second "base made, CA 0.13 s, denoiser cached".
The 103 here is at full size where §167's 45 was at 2048.

**Not done.** The check read the learned denoiser's answer from its
disk cache in every run, so it says nothing about whether the network
itself agrees to the pixel between WebGPU and the CPU. A session whose
denoiser ran on WebGPU and an export whose pair is remade on the CPU
run the network on two providers; whether those agree is a separate
question from the CA's, and is not measured here.
