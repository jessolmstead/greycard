# 265. Nikon, Sony and Panasonic styles for the camera match (2026-10-08)

The style reader (§180) read Canon CR3 and Fujifilm RAF and named the
other three makers with no style, so the camera match left every NEF,
ARW and RW2 out. It now reads the three, each against exiv2 on real
files: the style, the settings that bend the rendering per scene, and
the camera's own vignetting correction. The tag numbers and value names
are exiv2's (`nikonmn_int.cpp`, `sonymn_int.cpp`, `panasonicmn_int.cpp`,
and `minoltamn_int.cpp` for Sony's scene modes), GPL-2.0-or-later, named
in the file header; the reading code is ours.

**The oracle.** Our own files are too uniform to check a table
on: 90 Z6 III NEFs (Picture Control Auto, Neutral and Standard, Active
D-Lighting off), 8 A7 IV ARWs (Standard, DRO Auto) and 8 S5 II RW2s
(Natural, Intelligent D-Range off). So we took the first megabyte of
283 raws from raw.pixls.us (CC0) across 26 Nikon, 25 Sony and 24
Panasonic bodies, from the D3 and the NEX-7 to the Z6 III, the A7R V
and the S1 II; the maker notes sit in a file's head, so the head is
enough for both readers, and two 96 MP RW2s whose embedded JPEG runs
past the megabyte were fetched to two. They gave the tables values
beyond the defaults: Active D-Lighting Off, Low, Normal and Auto;
Picture Controls Standard, Neutral, Flat, Portrait, Auto and two custom
ones (one built on Standard, one on Vivid); Nikon's vignette control
Off, Low and Normal; Creative Styles Standard, Vivid, Neutral and
Portrait; DRO Off, Standard and Auto; Intelligent Auto on with the
scene modes Auto and Auto+; vignetting correction Off, Auto and n/a;
Photo Styles Standard or Custom, Vivid, Natural and two codes exiv2
does not name; Intelligent D-Range Off, Low, Standard and High;
shading compensation Off and On. The check (`tests/camera_style.rs`,
run over a folder and frame lists) compares every field with exiv2's
own names (`-Pkt`) and prints a table per body; 389 files, every field
agreed, and the files' raws are not in the tree.

| Maker | Bodies | Files | Fields compared | Agreed |
|---|---|---|---|---|
| Nikon | 26 | 211 | Picture Control 211, base 52, style fixed 205, Active D-Lighting 211, auto contrast or saturation 32, scene mode 211, program 211, vignette control 211, note read 211 | all |
| Sony | 25 | 80 | Creative Style, style fixed, DRO, Auto HDR, scene mode, Intelligent Auto, Picture Effect, vignetting correction, note read: 80 each | all |
| Panasonic | 24 | 98 | Photo Style, style fixed, Intelligent D-Range, HDR, shooting mode, shading compensation, note read: 98 each | all |

(The per-body table is below.)

**exiv2 misreads Nikon's newer Picture Control records, and we read
them as written.** exiv2 reads tag 0x0023 with the first version's
layout (name at 4, base at 24, contrast at 51, saturation at 53). The
second version ("0200", the D500 to the D850) keeps the names there but
spaces the adjustments two bytes apart, contrast at 55 and saturation
at 59; the third ("0300" to "0310", the D6, D780 and every Z) writes
the version twice and moves the name to 8, the base to 28, contrast to
63 and saturation to 67. exiv2's name for a third-version record is
"0310STANDARD", its base empty, and its Contrast "Auto" on every one of
them (it reads a zero byte that is not the contrast). The layouts are
ExifTool's (PictureControl2 and PictureControl3), checked on the bytes
of the files: a D750's and a D780's Standard read sharpening 3, mid-range
2 and clarity 1 at the third layout's offsets, their defaults. The check
compares the name with exiv2's minus the second version copy, and the
base and the auto adjustments only where exiv2 can read them (52 and
32 files, the first and second versions). On the third version the
auto adjustment and a custom control's base were checked on the bytes,
not against exiv2: the D750's and D780's defaults at the offsets, and
the Z 6II's custom "VIVID-KR" built on Vivid; no sample has a contrast
or saturation at A.

**Tone settings and settings that replace the style.** The first
cut listed every adaptive setting alike, and the match's sample takes
frames with one on to fill a group short of twenty fixed frames
(§181). That is right for a setting that bends the style's tone per
scene, Active D-Lighting, DRO, Intelligent D-Range, Canon's ALO and
HTP, Fujifilm's DR: the frame is still the style's rendering, with a
lifted shadow. It is wrong for one that renders in place of the style
or draws over it: a scene or intelligent auto mode, a picture effect
or creative filter, an in-camera HDR. Two sample ARWs, an A6000's and
an A7C's, were shot on Intelligent Auto (scene Auto+ and Auto) and
keyed as "Sony Standard"; a group short of fixed frames would have
fitted its Standard on them. So each adaptive setting now has a kind,
tone or replacing, and a frame with a replacing setting on, or with
any setting not known, has no group key: it counts with the raws that
have no fixed style and is never sampled. A tone setting on keeps the
key, so the frame is of the group, not fixed, and still fills. The
not-known rule is new for Canon and Fujifilm too (a CR3 with no
LightingOpt was "Canon Faithful", not fixed, and is now of no group),
so the schema 7 step marks every row with a maker unread, not only the
three new makers'. On 471 Canon and Fujifilm frames of our own archive,
grouped before and after the change, no frame changed group.

**What each maker reads.**

- Nikon: the Picture Control's name (title case for a preset,
  "Standard", "Neutral"; a custom control's own name, with the preset
  it was built on as the base). **Auto is not a fixed style**: Nikon's
  Auto Picture Control adjusts hue and tone to the scene, the way
  Canon's Auto style does, so it and a custom control built on it get
  no group; a custom control is as fixed as its base, and one whose
  base is not written cannot be told and is not fixed. Adaptive:
  Active D-Lighting (Off, or Low to Extra High and Auto) and a
  control's Contrast or Saturation set to A, which also picks per
  scene, both tone. Replacing: the scene mode (0x008f, text, empty off)
  and the program the consumer bodies write when the dial is on Auto, a
  scene or an effect (VariProgram, 0x00ab: "AUTO" on two sample bodies,
  spaces on P, A, S and M), which is where an effects mode shows; no
  sample has a scene mode. A type 1 note (the first Coolpix raws), with
  none of these tags, is a maker with no style rather than an error.
  Vignetting: Vignette Control (Off, Low, Normal, High).
- Sony: the Creative Style string by exiv2's names ("BW" is "Black and
  White"); the Creative Look bodies write their looks under the same
  tag (ST as Standard, FL, IN, SH, VV2 as themselves). "None" is no
  style. A string exiv2 has no name for is `Unknown (…)` and not fixed.
  Adaptive: the Dynamic Range Optimizer (Off, or any level, Advanced or
  Auto), tone; replacing: Auto HDR, the scene modes and Auto, Auto+ and Superior Auto
  (which render by the scene in place of the Creative Style; SLT's
  Continuous Priority AE is a drive mode and is not one), Intelligent
  Auto, and a Picture Effect (Toy Camera, Posterization and the rest
  are drawn over the style). Vignetting: Vignetting Correction (Off,
  Auto, n/a). DRO missing counts as on; the bodies without Auto HDR, a
  scene mode tag, Intelligent Auto or Picture Effect do not write them,
  so their absence is off, as Fujifilm's D-Range Priority's is.
- Panasonic: the Photo Style code. exiv2 names 0 "NoAuto"; it is the
  style the intelligent auto modes write, which picks by the scene, so
  it is Auto here, as ExifTool names it, and not fixed. 1 is "Standard
  or Custom": the camera writes the same code for Standard and its
  custom styles, so they are one group under exiv2's name, fixed, and
  a group may mix them. Codes exiv2 does not name (12 and 16 among the
  samples; ExifTool calls 12 Like709 and has the cine and Leica styles
  beside it) are `Unknown (n)` and not fitted. Adaptive: Intelligent
  D-Range, or Intelligent Exposure on the bodies before it (one of the
  two must be there), tone; replacing: HDR, a shooting mode other than
  Program, Aperture, Shutter or Manual (every scene mode and the
  intelligent auto modes render by the scene in place of the Photo
  Style), and the Filter Settings (0x00a1), a creative filter usable in
  P, A, S and M too. exiv2 does not list 0x00a1 or print it, so its
  layout is ExifTool's (FilterEffect, two 32-bit words, both zero for
  off) and it was checked on the bytes: every sampled body writes 0, 0
  but the GH5 II, whose four frames write 0, 1, which ExifTool names
  Expressive on the GH6. Those four now have no group; they were four
  of the "Standard or Custom" frames.
  Vignetting: Shading Compensation.

**An RW2's maker note is not in its Exif IFD.** The RW2's own Exif IFD
has none; exiv2 reads the note from the EXIF of the JPEG the RW2 embeds
(IFD0's JpgFromRaw, 0x002e), with offsets from that EXIF's TIFF header.
The reader reads the JPEG's first 160 KiB, which holds its EXIF
segment, as it does a RAF's.

**A bounded IFD reader of our own for the TIFF raws.** rawler's
`Entry::parse` allocates what an entry's count asks (`vec![0; count]`)
before it reads, so a damaged count asks for up to 32 GiB, an
allocation failure the panic guard cannot catch. The NEF, ARW and RW2
paths, and the TIFF IFD0 every TIFF raw is named by, now go through a
small reader that reads one IFD's entry table (at most 1024 entries)
and a value only when it fits the file and 64 KiB; a value that does
not is not read, and the setting it would have given counts as not
known. Next-IFD chains are not followed, as before. The CR3 and RAF
paths still use rawler's parsers and so still carry that allocation;
that goes upstream with the next-IFD cycle.

**Existing libraries.** A schema 6 row of these makers was read when
the reader gave them no style, and the index does not read a row's
tags again until the file changes, and the grouping rule above
changed which frames group for every maker. Schema 7 adds nothing and
marks every row with a maker unread, so the next pass over their
folders reads their tags and nothing else of the files.

**End to end.** On our own files (hard links in a scratch
folder, every XDG directory redirected, no sidecar, no look store
touched), through the match's own survey, plan, run and fit. The
survey groups them as the sheet would: "Nikon Z 6 3 Neutral: 15 frames,
15 with the adaptive settings off", "Nikon Z 6 3 Standard: 3 frames, 3
with the adaptive settings off", "Panasonic DC-S5M2 Natural: 8 frames,
8 with the adaptive settings off", "Sony ILCE-7M4 Standard: 8 frames,
0 with the adaptive settings off", and the 72 Auto frames are "raws
[with] no fixed picture style and not grouped". Every group is under
twenty, so the run skips each one and writes nothing, as it should.
Fitted anyway, outside the run, on the sample the run would take: the
S5 II's Natural lands at ΔE 0.0032 fitted and 0.0047 held out over its
8 frames; the Z6 III's Neutral at 0.0154 and 0.0268 over 12 of 15
(three did not register, correlation 0.88, 0.58 and 0.58); the A7 IV's
DRO Auto frames at 0.0042 and 0.0061 over 7 of 8 (one at 0.70). All 15
Neutral NEFs decoded. The reader reads the maker note only and never
the raw data, so it read all 90 Z6 III NEFs whatever their compression. The Sony frames are not fitted on, but only because
eight is too few: with DRO on all of them the group has no fixed frame,
the line says "0 with the adaptive settings off", and the sample takes
adaptive frames when a group has under twenty fixed ones (§181), so
twenty-odd DRO Auto frames would be fitted, which is the tone
rule above at work. Run again after the kinds went in, every line and
figure was the same: none of our frames has a replacing setting
on. The line does not say
which setting kept them from being fixed; the index keeps whether a
frame is fixed, not why.

**Not read yet.** Sony's Picture Profiles, in the enciphered
0x9416/0x2010 blocks (exiv2's Sony2010e PictureProfile): a frame shot
under one may still group under its Creative Style; the guide says so. The per-style tweaks that change the rendering
without adapting to the scene, as for Canon and Fujifilm: a Picture
Control's sharpening, contrast, brightness, saturation and hue steps,
Sony's contrast, saturation, sharpness and the Creative Look's fade,
shadows and highlights, a Photo Style's own adjustments. Sony's
Creative Look and Creative Style share a tag and a name ("Standard"),
so a group is per body, as it is anyway. Panasonic's newer Photo
Styles (L.Classic Neo, the cine styles, Real Time LUT) are codes exiv2
does not name and are not fitted until it does or we take names from
another table. Never seen on a real file, so checked only on synthetic
notes: a Nikon contrast or saturation at A, Sony's Auto HDR and Picture
Effect on, Panasonic's HDR, the intelligent auto modes, and Intelligent
Exposure, and a Nikon scene mode.

### Per body

#### Nikon

| Body | Files | Active D-Lighting | Auto, Scene or Effects Program | Picture Control | Picture Control base | Scene Mode | auto contrast or saturation | maker note read | style fixed | vignette control |
|---|---|---|---|---|---|---|---|---|---|---|
| NIKON D3 | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| NIKON D500 | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | - | 6/6 | 6/6 | 6/6 |
| NIKON D5300 | 2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 |
| NIKON D6 | 6 | 6/6 | 6/6 | 6/6 | - | 6/6 | - | 6/6 | 6/6 | 6/6 |
| NIKON D700 | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| NIKON D7000 | 5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| NIKON D750 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | - | 4/4 | 4/4 | 4/4 |
| NIKON D7500 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | - | 4/4 | 4/4 | 4/4 |
| NIKON D780 | 4 | 4/4 | 4/4 | 4/4 | - | 4/4 | - | 4/4 | 4/4 | 4/4 |
| NIKON D800 | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| NIKON D850 | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | - | 6/6 | 6/6 | 6/6 |
| NIKON D90 | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| NIKON Df | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| NIKON Z 30 | 4 | 4/4 | 4/4 | 4/4 | - | 4/4 | - | 4/4 | 4/4 | 4/4 |
| NIKON Z 50 | 2 | 2/2 | 2/2 | 2/2 | - | 2/2 | - | 2/2 | 2/2 | 2/2 |
| NIKON Z 6 | 6 | 6/6 | 6/6 | 6/6 | - | 6/6 | - | 6/6 | 6/6 | 6/6 |
| NIKON Z 6_2 | 6 | 6/6 | 6/6 | 6/6 | - | 6/6 | - | 6/6 | - | 6/6 |
| NIKON Z 7 | 6 | 6/6 | 6/6 | 6/6 | - | 6/6 | - | 6/6 | 6/6 | 6/6 |
| NIKON Z 7_2 | 6 | 6/6 | 6/6 | 6/6 | - | 6/6 | - | 6/6 | 6/6 | 6/6 |
| NIKON Z 8 | 3 | 3/3 | 3/3 | 3/3 | - | 3/3 | - | 3/3 | 3/3 | 3/3 |
| NIKON Z 9 | 3 | 3/3 | 3/3 | 3/3 | - | 3/3 | - | 3/3 | 3/3 | 3/3 |
| NIKON Z f | 3 | 3/3 | 3/3 | 3/3 | - | 3/3 | - | 3/3 | 3/3 | 3/3 |
| NIKON Z fc | 2 | 2/2 | 2/2 | 2/2 | - | 2/2 | - | 2/2 | 2/2 | 2/2 |
| NIKON Z50_2 | 6 | 6/6 | 6/6 | 6/6 | - | 6/6 | - | 6/6 | 6/6 | 6/6 |
| NIKON Z5_2 | 6 | 6/6 | 6/6 | 6/6 | - | 6/6 | - | 6/6 | 6/6 | 6/6 |
| NIKON Z6_3 | 96 | 96/96 | 96/96 | 96/96 | - | 96/96 | - | 96/96 | 96/96 | 96/96 |
| **all** | 211 | 211/211 | 211/211 | 211/211 | 52/52 | 211/211 | 32/32 | 211/211 | 205/205 | 211/211 |

#### Sony

| Body | Files | Auto HDR | Creative Style | DRO | Intelligent Auto | Picture Effect | maker note read | scene mode | style fixed | vignetting correction |
|---|---|---|---|---|---|---|---|---|---|---|
| DSC-RX100M7 | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| DSC-RX10M4 | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| DSLR-A900 | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| ILCE-1 | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 |
| ILCE-6000 | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| ILCE-6400 | 2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 |
| ILCE-6600 | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| ILCE-6700 | 2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 |
| ILCE-7C | 2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 |
| ILCE-7CM2 | 5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| ILCE-7CR | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| ILCE-7M2 | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 |
| ILCE-7M3 | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 |
| ILCE-7M4 | 14 | 14/14 | 14/14 | 14/14 | 14/14 | 14/14 | 14/14 | 14/14 | 14/14 | 14/14 |
| ILCE-7RM2 | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| ILCE-7RM3 | 5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| ILCE-7RM4 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| ILCE-7RM5 | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| ILCE-7SM3 | 2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 |
| ILCE-9 | 2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 |
| ILCE-9M3 | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| NEX-7 | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| SLT-A99V | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| ZV-E1 | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| ZV-E10 | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| **all** | 80 | 80/80 | 80/80 | 80/80 | 80/80 | 80/80 | 80/80 | 80/80 | 80/80 | 80/80 |

#### Panasonic

| Body | Files | HDR | Intelligent D-Range | Photo Style | maker note read | shading compensation | shooting mode | style fixed |
|---|---|---|---|---|---|---|---|---|
| DC-G100 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DC-G9 | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| DC-G9M2 | 5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| DC-GH5 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DC-GH5M2 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DC-GH5S | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| DC-GH6 | 5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| DC-GH7 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DC-GX9 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DC-LX100M2 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DC-S1 | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| DC-S1H | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| DC-S1M2 | 2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 |
| DC-S1R | 1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 |
| DC-S5 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DC-S5M2 | 11 | 11/11 | 11/11 | 11/11 | 11/11 | 11/11 | 11/11 | 11/11 |
| DC-S5M2X | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 |
| DC-S9 | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 |
| DMC-FZ1000 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DMC-G7 | 6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 | 6/6 |
| DMC-G80 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DMC-GH4 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DMC-GX8 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| DMC-LX100 | 4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 |
| **all** | 98 | 98/98 | 98/98 | 98/98 | 98/98 | 98/98 | 98/98 | 98/98 |
