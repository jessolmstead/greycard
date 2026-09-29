# 182. A half-copied raw that decodes (2026-09-27)

**A half-copied raw that decodes.** §179's guard named a half copy
only when rawler panicked on one, and the ARW never does: 22 of its 25
zero-tailed cuts decoded to a picture blank below the copied part, and
a CR3 will do the same once dnglab/dnglab#851 is in a rawler release.
So the develop now asks the question itself. `decode_source`, which
`decode_bytes`, `decode_path` and `decode_path_with_metadata` all come
through, refuses a file whose last 64 KiB are zeros before it asks
rawler for a sample, with the same words the panic gets, less the
decoder's own: "the file ends in zeros where a finished one has data:
it looks half-copied, or still being copied". It costs no read. rawler
maps the whole file with its pages populated before it decodes, and
`decode_bytes` was handed the bytes, so the tail is already in memory;
reading it again from disk as the panic path does would be a second
open for nothing. It goes ahead of rawler rather than after a
successful decode because the answer does not depend on what rawler
makes of the file, and a panic it would have thrown on the zeros is
spared; the NEF, RAF and RW2 cuts now get the half-copied message
without "the decoder gave up" behind it. It is a check on the file,
not on any one format, so a CR3 that stops panicking after #851 lands
is caught by the same line, and so is whatever format turns out to
read zeros quietly next.

The probe, the stance and the camera preview do not ask. The probe
and the stance read the file's head, which a copier writes first, and
what they read there is the finished file's: the camera and the date
for the index, the orientation. The maker's JPEG can sit past the
copy point and come out partly grey while the copy runs, which §163
already meets: the thumbnail cache makes the picture again when the
copy moves the file's time. Refusing any of the three would put an
error on a frame that is fine a few seconds later. Only the develop reads the samples at the end, and only it can
come out wrong.

Refusing a successful decode is a stronger claim than naming a failed
one, so the claim was checked against every sample raw. None of the
74 (one ARW, 46 CR3s, four DNGs, three NEFs, 19 RAFs, one RW2) ends
in 64 KiB of zeros. The ARW ends in 2048 zeros, padding to its block;
the NEFs, RAFs, RW2 and DNGs have no run longer than about 950 bytes
anywhere in their last 64 KiB. The CR3s come closest: their last
100 KiB or so is the CTMD track, which holds a 78,904-byte run of
zeros on every body among the samples and ends in some 23 to 25 KiB
of records after it, so a CR3's last 64 KiB are 70 to 85% zeros with
a 40 KiB run at the start. That is why the test is the whole tail and
not a run in it or a share of it. The one finished file that could
fail it is an uncompressed frame whose last 32,768 samples are exactly
zero: a DNG from another converter with the black already taken off
and a clipped black foreground. No camera writes that, and the message
says what was seen, so a user who meets it knows why.

Only zeros count. A copier that sets the length first leaves the
rest as the filesystem reads it back, and a preallocated or sparse
region reads as zeros on every filesystem there is; nothing leaves a
tail of 0xFF or any other one byte. Widening it would buy no case and
cost a margin: the RAFs' last 64 KiB are 27 to 52% 0xFF.

The core's test writes a linear DNG of its own, pads it with a whole
tail of zeros past its directory (which rawler reads without
complaint, as it does the ARW, and the test checks it does), and sees
all three decode paths refuse it with exactly the half-copied words
while its probe and stance still read; the same DNG as written, and
padded one byte short of a whole tail, decode as before. By hand, a
copy of the sample ARW with everything past 50% and past 90% zeroed
now fails `greycard develop` and `greycard info` with "decode failed:
the file ends in zeros where a finished one has data: it looks
half-copied, or still being copied", and every sample raw that
decoded before still does.
