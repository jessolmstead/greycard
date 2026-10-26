# §435. A RAF is judged whole by its header

*2026-10-26*

**Reported.** Two testers' RAFs, from an X-T5 and an X-H2, were
refused with "the file ends in zeros where a finished one has data"
(issues #38 and #48, shipped in 0.8.1). The half-copy rule refuses a
file whose last 64 KiB are zeros, since a preallocating copier
leaves that until it is done and no raw on hand ended so.

**What our samples show.** Every RAF here (X-T5, X-E4, X100VI, and
GFX100S II) ends on its last byte of CFA data: the header's CFA
offset plus length is the file's size. So the testers' files carry
a tail past the data that ours do not, and the file's last bytes are
the wrong place to look for a RAF. Our samples padded with 128 KiB of
zeros decode as before; with the 64 KiB before the padding zeroed as
well, they are refused.

**The rule.** A RAF starts with `FUJIFILMCCD-RAW `, and at 0x54 its
header holds six big-endian u32s: the offset and length of the
preview JPEG, the metadata, and the CFA data. A RAF is half-copied
when a section ends past the file, told so in its own words ("the
file's header names data past its end"), or when the last 64 KiB of
its CFA data are zeros, told as before. One that holds its sections
and whose data does not end in zeros is whole, whatever follows. A
file shorter than the header, or without the magic, falls back to the
tail rule, as does every other format. Both places the rule runs use
it: the check before the samples are decoded, on the bytes in memory,
and the words for a decoder panic, which read the head, the tail, and
the CFA data's end from the file.

**What the data's tail keeps.** A preallocating copier gives a RAF its
final size first, so its header's sections fit while the copy runs.
rawler's Fujifilm decompressor reads zeros as valid codes, so such a
file would decode without complaint, flat below the copied rows, and
an uncompressed one black. The zeros at the end of its CFA data still
refuse it.

**What a tester's sample would still tell.** Where the zeros sit:
past the header's CFA length, which this fixes, or inside it, which is
still refused and would want a narrower rule. Also the tail's length
and whether anything sits in it, and whether the X-Trans 5 bodies
write it on every frame or only in some modes (compressed lossless,
HEIF beside, pixel shift).
