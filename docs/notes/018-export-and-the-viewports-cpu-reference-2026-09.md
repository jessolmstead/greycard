# 18. Export, and the viewport's CPU reference (2026-09-06, late)

The rule that every GPU operation has a CPU reference (§5 rule 4)
had a gap: the viewport shader (exposure, contrast, the tone curve,
the matrix to sRGB, the encoding) had no CPU twin. Export closes it.
`greycard-ui/src/finish.rs` is the same transform over a working
image on the CPU, with a test that pins where mid grey and white land
under the curve, that contrast leaves mid grey alone and moves a stop
above it, that a stop of exposure is a stop, and what the curve's off
switch does. The worker keeps the last developed working image (a
24 MP frame is 290 MB of floats beside the 190 MB of halves the GPU
holds; fine for now, a budget later), so an export under the same
develop settings is the finish and the encode alone; a different
develop runs first. `Export JPEG` in the panel writes
`NAME.greycard.jpg` beside the file (a name no camera writes), quality
the `image` crate's default, no embedded profile yet; `--export PATH`
does the same for the first file and quits, PNG by extension too.

Checked against the GPU: the viewport at 1:1 (`--zoom 1`) and the
export's matching crop differ by 0.12 of 255 on average, 0.2 percent
RMSE, once the crop is the average of the two rows the shader's
half-pixel sampling blends at an odd viewport height; the shader is
held to the CPU from here. Sidecars aside, this makes the UI crate a
complete round trip: a RAW in, an edit, a finished file out.
