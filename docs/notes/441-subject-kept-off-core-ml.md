# §441. Subject kept off Core ML

*2026-10-27*

§422 put Subject through Core ML with Sky and the denoisers, since its
shapes are fixed. On the M5 the compile fails, as the roadmap recorded
on 2026-10-25: ONNX Runtime 1.28's ML Program builder stops at
`/squeeze_module/squeeze_module.0/dec_att/aspp_deforms.2/atrous_conv/Conv`
with "Required param 'pad' is missing", and `open` falls to WebGPU.

What the roadmap did not record is how long the failure takes. Core ML
converts the whole graph before it reaches that Conv. With `probe`
(`PROBE_EP=coreml PROBE_ONLY=coreml`) on the WebGPU rewrite, it took
2 min 22 s to fail; the WebGPU session builds in 1.2 s. The first
Subject mask of a launch waited out all of it, with no word in the
window. 0.9.0 shipped that way; the fix is in 0.9.1.

`providers.json` remembers the failure, so the next launch skips it,
but an entry is trusted only for the same build and the same macOS
(`coreml_machine_identity`). So every new greycard and every system
update paid the two minutes again, as would each tester's first
Subject mask.

Subject now takes the window's providers through `subject::offered`,
`Provider::without_core_ml`, as SAM, SAM 3 and LaMa do. Both Subject
files are kept off, since the original has the same Conv. Sky and the
denoisers stay on Core ML, where they win the race.

Putting Subject back on Core ML waits on that Conv being exported with
explicit pads, or on an ONNX Runtime whose builder accepts it, and on
a compile that finishes in seconds rather than minutes.
