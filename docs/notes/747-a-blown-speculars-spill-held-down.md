# §747. A blown specular's spill, held down (2026-11-25)

§743 found the stack ringing a blown specular: the defocused frames'
wider discs have rims that are edges in their own frames, hard ones
where the disc is clipped, and those rims outvote the focused
frame's ground beside its smaller disc. Bands then come from
different frames' rims: contours at the wrong radii, and a ground
half as bright where the bands disagree. The prior (§740) gave the
blocks beyond the disc to the most defocused frames.

**The hold.** Light is conserved: where a frame stands brighter than
the darkest frame does at a place, and that excess is at the clip,
joined to a clipped source, and the darkest frame's is not, the
frame is showing spilled light there, not the scene. Its weights are
held to a millionth at every level of the pyramid, through 8 px and
ramped back over 8 more, and the prior leaves it out of a block's
peak. The frames are leveled by a median offset and compared at
quarter size, the bar raised by the frame's own noise.

**Measured**, on a specular at six times the clip in focus in one
frame of nine, through the real walk: one edge at 8 px of 1.93
stops a pixel and a ring at 0.90 of the ground, where the focused
frame alone has 1.93 and 0.90, and the merge had edges at 9, 11, and
14 px and a ring at 0.50. On the core's bed, 5, 9, and 13 frames:
the edge from 1.14, 0.83, 0.79 to 1.82, 1.75, 1.30 of the focused
frame's 1.82; stray contours from as much as 1.23 stops a pixel to
0.07.

**Only at the clip.** A stack with no frame at its clip gathers
nothing, costs nothing, and merges as it did, to the bit. With a
clip somewhere, seven ordinary scenes differed from the old merge in
no pixel away from it. A stack with a clip takes 13 to 15% longer
and holds six quarter-size planes, 1.5 bytes a pixel.

**The wider hold we took out.** A first form held any spill, clipped
or not, and also mended the glow round a bright unclipped object and
the black middle of a dark object on a bright ground. Three reviews
each found ordinary scenes it harmed: bright specks beside a
brighter sky, then bright hairs crossing at two depths, held along
their length. The test compares frames at quarter size, where thin
bright structure averages down, so a crossing in the darkest frame
reads as a source; no gate we tried told the two apart. It is out
of the tree.

**Made again.** The recipe says whether the spill was heard
(`spill=heard` or `quiet`). A stack made again gathers again only
then, or where the prior speaks; one made before this has no key and
is made again without the hold, the file it was.

**Left.** The clip is told as a plateau at the frame's brightest
luminance, all three channels at once: of 83 real raws through the
placing read it fires on 37, on 24 of the 30 with a visibly blown
area, on no X-Trans frame, and where it fires the plateau is about a
fifth of the area clipped in some channel. Widening the plateau to
0.02 stops fired on frames with no clip at all. The glow round an
unclipped bright object (0.16 stops at nine frames) and a dark
object's middle remain.

No focus stack of real frames at one exposure has been through it.
