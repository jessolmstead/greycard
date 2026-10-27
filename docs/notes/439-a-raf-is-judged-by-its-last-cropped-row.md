# §439. A RAF is judged by its last cropped row

*2026-10-27*

**The numbers came back.** The X-H2 tester's file (issue #48): size
87416832, CFA data at 5608960 for 81807872 bytes, ending on the file's
last byte, and the last 64 KiB zeros. So §435's rule, which refuses
zeros inside the header's CFA length, would have refused it still.

**Why a whole file ends in zeros.** The file is the uncompressed kind.
rawler's data for that mode gives the sensor as 7872 × 5196 rows of
16 bits, 81793024 bytes, the CFA length less a 14848-byte header, so
the section holds the sensor and nothing more. But the active area is
5184 rows from row 5, ending at 5189, and the camera writes the seven
rows below it as zeros: 110208 bytes, more than the window looked at.
Every uncompressed X-Trans 5 HR file ends so by design. Our samples
(X-T5, X-E4, X100VI, and GFX100S II) are lossless compressed, whose
coder does not emit those rows, which is why neither rule ever saw it.
Issue #38, an X-T5 with the same sensor, reads the same.

**The rule.** A RAF's bytes cannot say whether its zeros are dead rows
or an unfinished copy, so the frame says it after the decode. The
decoded frame keeps the raw sample values, before the black level comes
off, and its crop is the camera's picture inside the sensor. Under a
black level near a thousand a real row never reads all zeros, so a last
cropped row of zeros is unwritten data: a copy that stopped before it.
The rows below the crop are never looked at. Up front a RAF is refused
only when its header names data past the file's end; a file that is
not a RAF keeps the tail rule as it was. The words for a decoder panic
still name a half copy when a RAF's CFA data ends in zeros, since a
whole uncompressed file decodes without one.

**What it misses.** A copy that stopped inside the last cropped row
passes, as the byte rule passed it; the picture is whole but for part
of a row. A tester's count of the trailing zero bytes, expected 110208,
would confirm the dead rows before the release.
