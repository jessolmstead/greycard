# §423. Camera profiles by the body's other names

*2026-10-24*

A DCP in the profile folder was listed only when its folded
`UniqueCameraModel` equaled the frame's folded make and model, or model
alone. Adobe names a body by its US name: the EOS 850D's profiles say
"Canon EOS Rebel T8i", so every one of them was hidden on an 850D
frame.

rawler's `Camera` keeps no aliases: its loader turns each entry of a
camera's `model_aliases` into a camera of its own and forgets the link.
It does publish the table it builds from (`decoders::CAMERAS_TOML`), so
greycard-core reads the aliases from that once, and a decoded frame and
a probe now carry `other_names`: the clean names the same body is sold
under, its own included when the file names an alias. Nothing else
about the camera changes; the core stays a list of strings.

The fit is one rule, in `camera::Entry::fits`, used by the panel's
list, its warning, a sync, a preset, and the import's preset check: a
profile naming no camera fits everything, as before; otherwise it fits
when its folded camera equals the folded make and model, model, make
and other name, or other name. Equality still, so "EOS R" never takes
an "EOS R5" profile. The hidden-profile log line now lists every name
tried.

We considered a prefix fallback for phones, whose profiles may name
the camera module after the model, and dropped it: it would offer an
"EOS R6 Mark II" profile on an R6, or a "Pixel 9 Pro" one on a Pixel 9,
as the camera's own, which is worse than hiding it. The limit: a phone
profile named for a lens ("Pixel 9 back camera ...") still hides until
we see the real names a tester's profiles carry.

## Review

A fresh review found the picture probe, which carries the EXIF make,
missing an alias on the five bodies whose EXIF make differs from the
clean one, so the panel listed a profile that a sync or the import
then left off; the names now come in both spellings. The prefix
fallback was dropped before review, on the parent's call.
