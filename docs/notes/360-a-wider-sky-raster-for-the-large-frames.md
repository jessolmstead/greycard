# §360. A wider Sky raster for the large frames (2026-10-18)

A mask's raster, the per-pixel matte the develop reads, was 2048
texels wide for every frame. On a 100 MP frame a texel covers four
to six frame pixels, and at 1:1 the fine edges, twigs, wires and leaf
rims against the sky, carried a light rim where the matte had been
averaged across them. §345 named a 4096-wide raster for Sky as the
remedy, and this is it, measured on a 100 MP frame copied for the
purpose and on 45 and 24 MP frames beside it.

## The rule

A Sky gets a 4096-wide raster when the frame, full size and as
turned, is wider than three times 2048, and the wide raster stays
within the 8192 texels every device guarantees. Every other shape,
Foliage included, stays at 2048. The line is drawn at how many frame
pixels a texel covers, not at a megapixel count: 24 MP frames at
6000 wide keep the narrow raster at 2.9 pixels a texel, 45 MP frames
upright too at 2.7, while 45 MP frames as shot at 4.0 and 100 MP
frames either way at 4.3 and 5.7 take the wide one, and so do the
24 and 26 MP bodies that are 6240 wide, at 3.05. A turn can move a
frame across the line, and a turn already remakes the raster.

Measured against a matte made at full size, over the band where the
matte is partial, the mean error on the 45 MP frame as shot halves,
and the share of pixels off by more than a tenth goes from 4.5 to
0.8 percent; the 100 MP frames improve less in the numbers and more
to the eye, since their rims were the widest. At 24 MP the numbers
improve a little and the crops look the same; the halo left there is
the matte's own.

## The cost

The matte runs on the 2048 preview whatever the raster's width, so
the growth, the pockets and the trimap cost the same; only the last
pass, the matte brought to full size and averaged into the raster,
sees the width, and it writes straight into the output now rather
than through a second copy, so the whole takes the same 0.3 to 0.8 s
and about 15 MiB more at its peak. Turning the matte into the raster
and the cache's PNG goes from 8 to 30 ms. The develop reads the
raster per pixel on the CPU and on the GPU alike, so its cost does
not change. A 4096-wide matte is not kept for Foliage, which asks
for 2048 and would never read it; a Sky and a Foliage on a frame
past the rule make two mattes, once per frame and turn.

On the GPU the rasters are an array sized to its largest member, so
one wide Sky would have made every brush and mask layer four times
larger; a Sky with ten masks on a 100 MP frame would have held
246 MB. The wide rasters go in a second array of their own, the
shape carrying a flag that says which, and the same edit holds
78 MB. The shader reads each raster within its own size, which also
fixed narrow layers being read at the array's size.

## The cache and the key

The mask cache keys a raster by its width when the width is not
2048, so wide rasters get a slot of their own and every narrow raster
keeps the one it had. The raster's rows come from the frame's size
on every path, the index's pass over a binned picture, the reduced
develop and the open frame, since the binning's rounding at 4096
reaches two rows on some frames (a 6240×4160 X-Trans frame, binned
by three) and a slot written with one height and read with another
would never read back. A frame past the rule that has only its old
narrow raster is not reported ready, which asks the frame's size of
its header before the pictures are read. The develop key stays as it
was: it is built from the sidecar alone, so it cannot see the frame's
size, and a bump would remake every Sky frame's pictures; the
pictures already kept for a frame past the rule stay until it is
opened or edited, a fraction of a level different at their size.

## Left

The header read for the frame's size maps the whole file, a full
extra read on a network share when no decode follows. If a format's
header crop ever disagreed with its decoded crop across the 6144
line, the index would make the narrow raster and keep asking for the
wide one; not observed. Narrow slots the index once wrote a row off
are remade once on their next read. The matte's stated working set
is the expected one under rayon's scheduling, not a hard bound,
since the guided filter nests its own parallelism inside every block.

## Tests

The rule's line, with the 6240-wide case; a matte twice as wide
averaging to the narrow one; the matte's expected working set; a
binned picture's raster having the frame's rows on both frame sizes,
read back through the index's own path; a wide frame's sky kept in
a slot of its own and not ready from the narrow slot; a wide sky not
kept as Foliage's cut; wide rasters beside narrow ones drawn by the
GPU as the CPU draws them, two of each interleaved, on NVIDIA and on
lavapipe within 0.0022.
