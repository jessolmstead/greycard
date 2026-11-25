# §740. The stack's coarse word on faint texture (2026-11-24)

§739 gave the stack's frames the full develop. A tester's macro
stacks were still soft beside Helicon Focus wherever the subject
holds only a faint, fine texture. The blend's weights are the cause.

**Why it averaged.** The weights are each frame's Laplacian energy
at full resolution. The Laplacian weighs a frequency by its square,
so the sensor's white noise outweighs a faint texture some octaves
below it. On the bed (base ISO, a texture of 1.5% of its level,
eleven frames) the focused frame took 0.095 of the weight where an
even split is 0.091: the stack was averaged, and a quarter of the
texture kept.

**The coarse word.** A level or two down the pyramid the noise has
averaged away and the texture has not. So each frame's energy is
also taken at half and quarter size over 16 px blocks, and where
one frame plainly stands out of a block, the full-resolution
weights of the others are scaled down by its share
(`stack/prior.rs`).

**Noise is never believed.** What decides whether a block's
sharpest frame is trusted is a ratio: the block's energy at the
reduced level over the same frame's full-resolution energy there.
On noise alone that is the noise's spectral shape, whatever its
level, so a darker frame or a deep shadow does not pass for a sharp
one, and no noise model is needed. The frame must stand some
multiple of the block's median over the stack, the multiple fitted
over 102,000 blocks of pure noise at each count from 8 to 250
frames and holding a margin of 1.1 to 1.3 over the largest seen. A
second simulation, written apart from the code, trusted nothing on
other seeds. Blocks too noisy to read (deep read-noise shadows) and
stacks under eight frames give no evidence, and merge as before to
the bit.

**Made again.** A stack made again from its kept places (§739)
gathers the evidence again, each frame read the short way first in
the chain's order, so the picture is the first merge's to the bit.
That is one more read a frame, for stacks of eight or more.

**What it gives.** Faint texture kept, on the bed:

| Stack | Before | After |
|---|---|---|
| 11 frames | 0.24 | 0.57 |
| 31 frames | 0.12 | 0.46 |
| 11 frames, four times the noise | 0.16 | 0.20 |

Strong edges and flat noise are unchanged, and halos did not grow.
The merge takes about a fifth longer in a debug build.

**Left.** A frame two stops from the rest shifts the weights among
frames holding the same texture, by up to 2.2 times; bounded, and
no seam. A noisy stack gains little: the price of believing no
noise. No real stack of eight frames or more has been through it
yet; both are on the roadmap.
