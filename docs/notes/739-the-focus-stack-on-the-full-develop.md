# §739. The focus stack on the full develop (2026-11-24)

§573 left the stack on the short develop the command line began with:
bilinear, no CA correction, highlights clipped. A tester's macro stacks
beside Helicon Focus showed what that costs: fine, faint texture, and
color fringes at high magnification. The stack now blends frames
developed as the HDR merge's are. (§738 is the collection menu's note,
landing beside this one.)

**Two reads, two develops.** The core reads each frame twice, once to
place it and once to blend it. `focus::frames` tells the passes apart
by the core's own steps.

- Placing takes a short develop: RCD on Bayer, Markesteijn in one pass
  on X-Trans, hot pixels repaired, no CA, highlights clipped. Measured
  on real frames against fits made on walked frames, bilinear placed a
  well-linked frame 0.25 px off at the corners; RCD lands within
  0.04 px, at 0.35 s a frame against a walk's 0.65 s.
- Blending walks each frame as the HDR merge does
  (`Purpose::MergeInput`): the edit's demosaic for the sensor, hot
  pixels, every frame at the reference's white. Of the edit only the
  prepare, the demosaic, the dual contrast, the CA, and the hot pixels
  reach a frame; no crop, lens, retouch, mask, look, or tone.
- One CA fit, made on the reference and applied to every frame, as a
  bracket's is. Lateral CA follows the image radius and moves little
  with focus; a fit on a half-blurred frame would differ more. The fit
  is made by the prepare and the fit alone, not a whole walk, and not
  at all where the reference's list holds none.

The command line walks under the default edit; the window under the
reference frame's, as Merge to HDR does.

**The cost.** Five 24 MP frames: 5.2 s to 9.3 s, about 1.8 times.
Peak memory 99 to 103 bytes a pixel, still one frame's working set
whatever the count; the refusal reads 115. With a learned tier asked
the walk holds the network's planes too: 169 to 171 bytes a pixel
measured on the CPU provider, and the refusal reads 190 then.

**Highlights stay clipped, for now.** Reconstruction needs headroom,
so the file's samples are stored that many stops darker, and the
capture sharpen reads absolute levels: on a file with headroom it
takes the shadows as darker than they are and sharpens them less
(the same samples with and without 1.55 stops of headroom: mean
|Laplacian| 11 to 31% lower with, identical with Sharpen off). That is
the sharpen's bug, and an HDR merge and a panorama have it today. Once
the sharpen reads such a file at the scene's levels the stack can keep
its highlights.

**The file.** It carries the reference frame's NoiseProfile: where the
detail is, a band comes from one frame and so does its noise; in flat
areas it overstates, which only smooths what has nothing to lose. And
it carries a recipe, as a bracket and a panorama do: the reference,
the CA, the tier, the selectivity, the residual, each frame's
transform or that it was dropped, and whether the samples were stored
uncompressed. File names only, no paths.

**Merge again.** A stack made again blends each frame at the
transform the recipe holds (`stack_camera_placed`), nothing fitted
again: with no tier it is the same file bit for bit. The Noise row's
"Denoise the frames and merge again" and `greycard merge again
--denoise` reach a stack, the learned denoiser a frame at a time, off
by default.

## Left

- Reconstructed highlights, after the sharpen's fix.
- No X-Trans stack test: the synthetic DNG writer writes Bayer only.
- The learned tier's memory on a graphics card, unmeasured; 190 is the
  CPU provider's figure and an upper bound.
- exiv2 warns "Directory Canon with 29287 entries" on any file with
  our DNGPrivateData, a merge's too: it reads `gr` of `greycard\0` as
  an entry count. No signature the DNG spec allows avoids it.
