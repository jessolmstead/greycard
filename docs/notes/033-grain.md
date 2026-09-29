# 33. Grain (2026-09-07)

Our list had it too. Grain is a noise over the frame as shown,
its cell in thousandths of the frame's width, so a size looks the same
at any size of export and follows the crop; it goes on last, on the
encoded output, as luminance, weighted towards the shadows (one minus
half the luma), as film's does. `Grain { amount, size }` in
greycard-edit's `grain.rs`, global like the vignette, with the noise
itself: an integer hash of a lattice point and a seed, value noise
between the lattice's hashes by a smoothstep, two octaves (the second
at twice the frequency, a third of the weight, offset so the lattices
do not line up), and `Grain::apply` laying it on. The shader carries
the same hash in u32 arithmetic and the same blend, so the two paths
draw the same grain: the check of §18 at 80% agrees to 0.065%, no
worse than without it. At the fit zoom the viewport minifies the
grain and it mostly vanishes, as it does in any editor; 1:1 shows it.
Chroma grain and a roughness control can come if wanted; the full
amount adds at most 0.12 to an encoded value, one constant to trim.
`finish_with` takes one closure for a pixel's place in the frame,
giving the vignette's stops and the grain together.

### Crystals, cubic and tabular

Our ask, before looking: it should be organic and filmic, a
non-repeating pattern of shapes, not a noise laid over a digital
photo; and could the grain be cubic or tabular, as film's crystals
are. Value noise was neither, being bumps on a lattice, so the grain
is now scattered crystals: every cell of the frame holds a few, each
at a place, with a radius, a signed strength and two dye tints all
cut from the bits of one hash of the cell and its index, and a pixel
sums the soft bumps ((1 − q²)² of the squared distance over the
radius) of the crystals of its cell and the eight around it. Nothing
tiles: the hash is of the cell's index, so each cell is its own. The
tints put a little color into each crystal, red against blue and
green against the rest, as a scan of color film shows dye clouds.
Two kinds: cubic, the classic emulsion, two crystals a cell of every
size from a third of a cell to nearly one and every strength, lumpy,
dyed at 0.3; tabular, the flat T-grain, three a cell of nearly one
size and strength, finer and more even, dyed at 0.15. `Kind::character`
holds the numbers and the shader takes them in two vec4s. The
weighting towards the shadows is now one minus 0.65 luma^1.5, so the
highlights keep more than they did. The test asks for zero mean, an
RMS about the amount, no repeat one cell or a hundred along, differing
channels, and that tabular changes more over a quarter of a cell than
cubic for its spread. The two paths still agree to 0.065% at 70%; a
45 MP export takes about five seconds in all with it on.

### Crystals that stay put

Our nit: the size slider slid the pattern away from the top
left, since the lattice was of the size and scaled about the frame's
corner. Now every crystal has a home in the finest lattice (a tenth of
a thousandth of the width, the smallest size) and keeps it. Coarser
lattices are the finest one halved in each direction per level, and a
cell at any level has four candidates: the finest cell's four slots
at the base, or its four children's picks above, where a cell's pick
is one of its candidates by a hash of the cell and its level. So a
crystal's place never depends on the size; the size chooses the level
(the octave of the size over the base) and how far through it the
size is, and grows the radius with that. Across the octave the three
candidates a coarser cell will not pick fade by (4/f² − 1)/3, so the
coverage holds and, at the crossing, the survivors are exactly the
coarser level's candidates: nothing pops. The test correlates the
field at one size with a third more and with one past the octave,
and wants both well above nothing (they were nothing before). The
neighborhood stays three by three at the size's level, radii being
at most half a cell; the cost is the descent, a hash per level per
candidate. Both paths agree to 0.065% still, and the export is a
half-second slower.
