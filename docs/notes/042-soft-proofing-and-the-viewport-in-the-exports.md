# 42. Soft proofing, and the viewport in the export's space (2026-09-15)

The first of the editor list. §17 left the display table as sRGB to
the monitor; the roadmap's line was "a second profile in the chain
with an intent and a gamut warning; the display table is the other
half". Both halves are in.

**The chain.** Working space, then the output's matrix and the sRGB
curve, then the table to the monitor. The output's matrix used to be
sRGB's whatever the export sheet said, so a P3 or Rec.2020 export
was never seen before it was written; now the renderer's matrix is
the sheet's space (`Renderer::set_output`), the histogram, scopes and
clipping marks are of that space's encoded values, which is what the
file will hold, and the table is built for it: `Lut3d::build(output,
monitor, proof)` through Little CMS from the space's own profile
(`Space::icc`, §20) to the monitor's, or to sRGB when colord knows
none, relative colorimetric. The identity only when the output is
sRGB and there is no monitor profile. The point of a correct chain is
that changing the export space changes nothing on a profiled monitor
until a color falls outside it; and that is what the screenshots
show. The table is rebuilt when the space or the proof changes, a
few milliseconds for a matrix profile, longer for a CLUT one, and
tells the scopes the picture is new, since they bin after it.

**The proof.** A SOFT PROOF section at the end of the Develop tab:
on or off (S), the profile (sRGB, Display P3, Rec.2020, or a file
through the desktop's chooser, opening in `~/.local/share/icc` or
`/usr/share/color/icc`), the intent, and the gamut warning. With a
proof the table is Little CMS's proofing transform: output to the
proof's profile under the chosen intent (perceptual, or relative
with black point compensation), then relative to the monitor, with
`SOFTPROOFING` so the proof device is emulated. For the three
spaces, matrix profiles, perceptual falls back to relative inside
Little CMS; the choice matters for a printer's profile, which is the
case the file option exists for. The profile and intent are kept in
the settings; whether the proof is on is not, since a proof is a
look at one export.

**The warning.** Little CMS marks out-of-gamut colors in a proofing
transform with `GAMUTCHECK` by painting them the alarm color, and
gives no other word. So the grid is transformed twice, with the flag
and without, the alarm set to a value no transform would land on,
and an entry that differs is out. The mark rides in the table's
alpha, which was unused; the shader paints mid grey where the
sampled alpha passes a half, before the clipping warnings of §41. A
test sends Rec.2020's grid through a proof to sRGB and checks the
marks against the matrix: every point whose linear sRGB is well
inside the cube unmarked, every point well outside marked, with
Little CMS's own tolerance at the edge. A Gray profile marks
everything, as it should; ghostscript's SWOP CMYK, on a muted frame,
nothing, and lifts the blacks as SWOP does.

**What it is not.** No paper simulation (absolute colorimetric to the
monitor, the paper's white shown as a tint); the intent to the
monitor is relative. No per-channel gamut mark. The histogram's end
bins are of the output space, so the clipping marks of §41 say the
export clips, which is the right question; the proof's clipping is
the warning.
