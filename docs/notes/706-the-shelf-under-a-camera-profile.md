# §706. The shelf under a camera profile (2026-11-16)

A tester's log (discussion 168) showed every return to a frame
developing in full: no "from the shelf" line, and the shelf growing by
a base at each return. Their profile folder holds 4,439 profiles.

**The cause.** The steps to a base (§378) compared the camera profile
by pointer, on the grounds that a profile is read once and kept. It is
kept only until `profiles::list()`, which empties the loaded-profile
cache, and the window lists on every open. So under a named DCP the
return to a frame asked with a new pointer and never matched its
shelved base. Any listing while a frame was open made the base in hand
stale as well, and the next slider move rebuilt it.

**The fix.** Profiles compare by pointer first, then by content
(matrices, maps, look table, and curve; about 3 MB at most, 3 to 3.5 ms
in a debug build, reached only when the pointers differ). A profile
rewritten with other tables is another base. A base made again for the
same file under the same edit now replaces the old one.

**Stand-ins.** §378 never shelved a base standing in for the learned
denoiser, so a return under a missing model paid the whole develop.
Now the open asks whether the walk would stand in from its first step
(the model not in the store, or no runtime). A stand-in base is shelved
only while the leaving edit would still stand in, and taken only while
the coming one would. Once the model arrives it is not taken, and the
develop runs the model. One made with the model there (the card short,
or a run failed) is not shelved. A stand-in and a learned base of the
same file and noise section replace each other, the newer staying.

**A matrix that is not all numbers.** A DCP color matrix holding a NaN
or an infinity loaded, developed NaNs, and was unequal to itself, so it
missed the shelf forever. `dcp::mat3` now drops such a calibration as
it does one that does not invert; a forward matrix is dropped alone.

**Tests.** A DCP written by the test, A then B then A through
`swap_base` with the cache emptied after each open: A comes from the
shelf, bit for bit a fresh develop's, one base a file. The base in hand
outlives a listing and a slider move. Stand-ins: taken only while
standing in, refused when the card was short or a run failed.

**Limits.**
- A profile rewritten with the same tables but another name or
  copyright is another profile.
- A NaN inside a hue and saturation map or a look table compares equal
  to itself since 2026-11-19, so such a profile finds its base; only a
  profile built in code can hold one, the reader refusing them.
- No test covers the stand-in and learned replacement in `put`; it
  only evicts, so the cost of a slip is budget, not a picture.
- (An edited neighbor is developed ahead since §707, and the window
  no longer walks the profile folder on an open since §708.)
