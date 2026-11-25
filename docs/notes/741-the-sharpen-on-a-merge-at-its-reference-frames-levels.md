# §741. The sharpen on a merge, at its reference frame's levels (2026-11-24)

A merge stores its samples divided by its headroom, so the file's
white stands above the reference frame's by that many stops and the
finish brings the picture back up. The capture sharpen runs before
the finish and read three things at the file's levels: the L\* behind
its automatic threshold and its blend mask, the black pedestal, and
the floor of its early stop. On a merge all three were read darker
than the scene by the headroom, so shadows were taken for deeper
than they are and sharpened less.

**The scale.** `SharpenOptions::level_scale` is two to the headroom
the finish shows the merge at (`baseline::shown`, hundredths of a
stop): L\* is read at the level times it, and the pedestal and the
floor are divided by it. CPU and GPU take the same four terms. The
walk, the masks' planes, the command line's develop, the export
sheet's sharpen on a resize, and a mask's Detail delta (about the
same pedestal, `DetailMeta::pedestal`) all take it. With no headroom
every term is a multiply or a divide by one, and a file that is not
our merge renders as it did, to the bit: only our own merge tag gives
a headroom.

**Measured.** What the sharpen adds, as the mean Laplacian with it
on over with it off, on 400 px crops of two real merges beside their
reference frames:

| Crop | Before | After | Reference |
|---|---|---|---|
| HDR merge (3.06 stops), shadow | 1.16 | 1.88 | 1.79 |
| HDR merge, midtone | 1.76 | 2.07 | 1.90 |
| Panorama (1.07 stops), shadow | 1.56 | 1.68 | 1.68 |
| Panorama, midtone | 1.72 | 1.72 | 1.70 |

**Process 3.** A merge with headroom renders differently, so
`PROCESS` is 3 and every edit takes it, as with §735's step to 2. The
edited pictures' and the reduced develop's recipes are raised and the
old ones retired, so each file's pictures made from an edit are made
again once. The develop recipe is not: the masks' rasters and the
fills are taken before the sharpen. The render gate holds two new
cases, a merge with headroom and the same under a mask's Detail
Sharpen; every other kept picture is unchanged.

**Left.** Three more stages read a merge's levels as the file's and
are on the roadmap: Clarity's midtone fades, Dehaze's guide epsilon
(its variance term is off by the headroom squared), and a luminance
range mask's lightness. The stack's reconstructed highlights waited
on this and can follow.
