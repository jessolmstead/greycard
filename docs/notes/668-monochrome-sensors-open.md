# §668. Monochrome sensors open (2026-11-14)

A tester's Ricoh GR IV Monochrome DNGs were refused: "Linear layout
with 1 samples per pixel" (issue 146). DNG writes a monochrome sensor's
frame as `LinearRaw` with one sample a pixel; we mapped every
`LinearRaw` to the three-channel linear layout, and the develop refused
the monochrome layout besides. No monochrome DNG had ever opened. Two
Leica Monochrom files (CC0, raw.pixls.us) failed in the tester's words;
we have no GR IV file yet.

What a monochrome frame is to us now:

- **Decode.** `LinearRaw` with one sample, and rawler's `BlackIsZero`,
  are `SensorLayout::Monochrome`. As shot is the neutral `[1, 1, 1]`
  whatever the file says.
- **Color.** Its profile is `color::monochrome_profile`: one D65
  calibration whose matrix is XYZ to the working space, so the camera
  is the working space. `profile_from_frame` and `CameraWhite::profile`
  give it; nothing else builds a profile from a frame's calibrations.
- **No white tints it.** `resolve_white_balance` gives the neutral
  whatever the edit's white and the profile, on the CPU and the device,
  in the slider preview, and in the pickers; a mask's white is
  `WhiteShift::neutral`. The edit keeps the white it holds, so a synced
  or pasted white does nothing here and survives the trip. Color
  grading, the mixer, and looks still tone the picture. The panel's
  Temperature and Tint are not dimmed; that is a UI decision not yet
  made.
- **Develop.** The plane is copied into three equal channels after the
  levels, the crop, and the mosaic opcodes, and from there it is a
  linear frame to everything: the reduced develop, the walk, the device.
  Highlight reconstruction and the mosaic's CA correction have nothing
  to work from and are skipped.
- **Lens.** A lateral aberration correction would part the equal
  channels into fringes; `Lens::over_monochrome` turns it off.
  Distortion and vignetting run.
- **Noise.** The plane is measured and its hot pixels repaired as an
  RGGB mosaic (neighbors two apart are neighbors two apart), the three
  lines pooled into one. A merge of ours keeps its own `NoiseProfile`
  and gets no repair, as a color merge does.
- **DNG out.** One plane of `LinearRaw`, no `AsShotNeutral`, no
  `ColorMatrix`: the spec's form and what Leica writes. It reads back
  monochrome.

## The denoise took three tries

1. Three equal channels through the color path, the model at three
   times a photosite's variance so the luma had unit noise. The
   unbiased inverse then corrected for three times the real noise and
   lifted the shadows: +7.7% at 0.004 on a real file.
2. The true model, the wavelets' luma scaled by 1/√3. On the default
   hybrid and on the means this changed next to nothing, and its test
   passed without it: the plane kept about twice a color channel's
   residual spread, since a patch distance over three identical
   channels has three times the variance of one over independent ones.
3. One plane, denoised as one (`denoise::denoise_picture`, flagged from
   the frame by `Prepared::monochrome`). The means and the wavelet
   chain take the channel count as a const generic; at three the
   arithmetic and its order are what they were. On the plane the means
   compare patches of about three times the pixels (3x3 to 5x5), the
   wavelets shrink it as luma with no chroma, and the hybrid takes the
   band ratios measured for what one channel's means leave. The plane
   is worked in the picture's own buffer.

Against one channel of a color frame with the same model (synthetic
flat planes), the plane's residual spread is 1.36 (wavelets), 1.61
(means), and 1.69 (hybrid) times; the second try gave 1.6, 1.95, and
2.07. About √3 is the best one channel can do at the same strength,
since a color channel's value is mostly a luma that averages three
photosites. The shadow shift under the means and the hybrid is 1.43 and
1.34 times a color frame's: the inverse's bias grows with the noise
left, and one channel keeps more. We accept it; the tests bound it.

On the real files (ISO 1600, default denoise) the block means shift
+2.3% and +2.2% at 0.003–0.005, +0.2% and +0.1% at 0.01, and nothing
above. R, G, and B are identical, denoise on or off; device against
CPU is 6e-7 of white. A 24 MP plane's denoise adds about 85 MB and
0.7 s, less per pixel than a color frame's.

Color files are unchanged: a Bayer and an X-Trans raw develop to the
same pixels as before, through the CLI with each denoise method and
through an export on the CPU, the device, and lavapipe.

## What the review found that is not this change

- The capture sharpen stops a 32x32 block's iterations the first time
  a margin goes under zero. Where the margin lands within rounding of
  zero the device and the CPU stop an iteration apart and the block
  moves by up to 3%. Any file can meet it; a monochrome plane's
  unsmoothed noise meets it more often.
- The shared noise estimate reads up to twice a plane's own
  neighbor-difference variance in deep shadows, on a Bayer frame as on
  these.
- Texture alone differs by 1.9e-4 between the device and the CPU on
  one of the files, in scattered pixels.

All three are on the roadmap. Untested: a camera match on a monochrome
body, and HDR, panorama, and focus merges of monochrome frames.
