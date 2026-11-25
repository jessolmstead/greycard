# §743. The focus stack's highlights, under headroom (2026-11-24)

§739 left the stack's highlights clipped: what a reconstruction keeps
over the clip a file can hold only under headroom, and the capture
sharpen read such a file as darker than it is. §741 mended the
sharpen, so the stack now keeps them as the HDR merge and the
panorama do.

**The rule is theirs.** The blend's frames are walked with the
reconstruction the edit asks for; the placing reads stay short and
clipped. The merge is held between zero and the brightest sample of
the frames blended, and its headroom is its brightest sample after
the white balance, never under one. The file stores the samples
divided by it, in 16 bits (about 14.5 under white at a stop and a
half), with the stops in the tag and `highlights=reconstructed` in
the recipe.

**What changes, and what does not.** The headroom is set by any
sample over 1 after the white balance, not by the sensor's clip. A
stack with nothing there is the file it was, to the bit. One with a
blown area keeps what the reconstruction made of it, and one with a
saturated red or blue that its gain lifts over 1 now renders that
color as the frame's own develop does; the old file cut it at 1.
Time and memory are as they were.

**Made again.** A stack whose recipe holds no `highlights` key was
made clipped and is made again clipped, the same file, as a missing
kernel reads as the one that merge ran. A fresh merge of the same
frames takes the headroom. Made again with the prior's evidence
regathered (§740), nine frames under headroom come out the same to
the bit. An older build reads the file and honors its headroom, and
would make it again clipped.

**A blown specular is no better selected than before.** On a stack
of nine frames at one exposure with a specular in focus in one, both
the clipped merge and this one take the defocused frames' wider disc
edges for detail, and ring the disc with a ground half as bright:
the selection's fault, and the prior's, which reads a defocused
bright edge as texture. The headroom adds nothing to it but 3% of
depth to the ring. It is on the roadmap.

No stack of real frames at one exposure with a blown area has been
through this; the evidence is the synthetic scene through the real
walk.
