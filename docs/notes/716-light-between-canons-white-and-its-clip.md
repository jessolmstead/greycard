# §716. Light between Canon's white and its clip (2026-11-18)

The white rawler gives a CR3 is Canon's specular level (14008 at ISO
100, 14888 above), and the R6 II and R5 II saturate at 16383 in every
channel. Clamping at the white threw away 0.14 to 0.23 stops before
the white balance and the reconstruction, and about 15% of what we
rebuilt as fully clipped still held a real channel.

**What we did.** The white stays the exposure scale. A frame gets a
saturation level beside it (`Levels::saturation`), and a
reconstructing develop normalizes up to `H = (S - b) / (W - b)` and
flags at `0.987 H` times each gain where it flagged at 0.987. The
all-clipped fill and the cut after it rise by `H`, so a blown core is
no darker than its recovered edge. Clip mode, merges, the
camera-space demosaic, and every other camera normalize at one as
before. `Prepared::ceiling`, and so the clip level the sharpen and
the local contrast fade by, are unchanged.

**Where `S` comes from.** Nothing in the file says it, so a table of
ours: `S = b + 0.955 (16383 - b)`, for a body, a shutter, and an ISO at
which a frame under a second was blown in every channel (20 or more
samples in the top 32 levels at each of the pattern's four positions)
among 838 CR3s: the R6 II mechanical at 100 and 320, first curtain at
100, 125, 160, 200, 250, 320, 400, 500, 1250, 1600, 2500, and 12800,
and the R5 II at 100, first curtain and electronic. No ISO between two
measured ones is trusted: at 640 and 800 the R6 II's green piled and
its red did not (at the top in 2 of 8 frames at 640, by at most 11
samples at 800), and at none of 640, 800, 4000, and 5000 did blue
reach it. A frame gets it only when its maker
note says highlight tone priority, long exposure noise reduction, dual
pixel raw, and HDR are off and it is the full frame; anything unsaid
gets none. White and pile do not move with the aperture.

**The margin.** Bracket pairs read linear within about 1% to 0.955 of
the range and 1.5 to 2.5% low past 0.96. At 0.98 the develop read
greens in that low band as light, which leans magenta; at 0.955
nothing past the linear range is read, and opposed flags at 0.943. It
keeps 0.167 stops at ISO 100 and 0.076 to 0.092 above, 0.04 less than
0.98. Of the 93 pixels newly magenta in frame 48's bokeh, 65 (70%)
are data inside the linear range that master cut; 28 sit at rebuilt
greens; none reads past 0.955. What is left is the light read through
the white balance, which no margin removes without removing the
headroom.

**Bit for bit.** A frame of the table with nothing at the old flag
takes no headroom after one look at its raw samples. The noise
estimate and the hot pixel repair read a copy cut at one, so the
noise model is master's on every frame. The CA's fit is global, so it
is measured on the mosaic the frame makes without its headroom, kept
as the sites the old develop rebuilt and their values; the CPU
resamples the frame's own mosaic only in the tiles those sites reach,
the device measures on one and resamples both, and a device that
declines or fails hands the CPU the same reference. Nothing moves
beyond 32 px of what the old develop flagged; the learned denoiser's
field carries a difference to 115 px, under 1e-3.

**Every camera.** Opposed finds its clipped photosites a row of blocks
at a time, in parallel: the same mask, its pass from 37 to 9 ms on a
24 MP frame. All 37 decodable non-Canon frames hash equal to master's,
and the same at 1 and 3 threads.

**Keys.** The develop key is made without a decode, so the headroom's
version (`decode::HEADROOM`) goes into the keys of every `.cr3` and no
other file: edited pictures, fill slots, mask base prints, and what
the index's keys are made under. Raising it clears the develop key of
every row of the library, every make's: each is made again from its
sidecar when its folder is next read, one sidecar read a row, and an
offline root's rows when it is back. A fill slot of an earlier version
is removed where it is looked up or written. No sidecar change.

**Limits.** Linearity measured at ISO 100 on one body. The four flags
were only ever seen off: a frame each with HTP, LENR, dual pixel raw,
and HDR PQ on would settle their indexes. A hot sensor's bell under
the clip cannot be read from the file. An Auto white balance can move
by a hair on a blown frame. Merges keep no headroom. A build of 0.11 or
earlier sharing the cache loses its CR3 fills each time this one looks
them up, and makes them again. RCD on a card parts from the CPU beside
any cut plateau, at one or not. rawler sizes a TIFF value by its count
before reading it (a flipped count asked for 28 GB). The modes are
read through a reader that checks the note first, the one the fuzz
target runs; the decoder's own walk of the box, which reads the same
note, does not check, and the fuzz target does not cover it. Upstream: CMP1's bit depth
on rawler's `RawImage` would let `S` come from the file.

**Known limits.**
- The mode reader is tested on maker notes we built, not on frames shot
  with highlight tone priority, long exposure noise reduction, dual pixel
  raw, or HDR PQ; a frame of each is still wanted.
- Only the bodies, shutters, and ISOs of the table get the headroom.

**A third body (2026-11-19).** The EOS 850D (Rebel T8i, Kiss X10i: a
cell's model now also matches the other names rawler gives the frame's
own body) clips at 16383 in every channel, harder than the R bodies:
97 to 99% of each position's top 32 levels sit on 16383 itself, with
no pile under it. First curtain at ISO 200 is in the table, 0.133
stops. ISO 100 is not: Canon's white there is at 0.70 of the range,
so the headroom would be 0.45 stops, three times any other row, and
no bracket has shown that band linear on this sensor. On one of three
frames it gave muted lavender in exactly that band (7,370 pixels with
green under 0.95 of the lesser of red and blue). It waits on a
bracket. `HEADROOM` is "headroom-3".
