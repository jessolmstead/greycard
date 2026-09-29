# 93. What the frame was shot at (2026-09-19)

The panel said a file name and nothing else about the picture, so
every question a photographer asks first — what body, what lens, how
fast, how wide, how sensitive — meant leaving the editor. v0.1.0 asks
for the line; most of what it needs was already decoded and one
field of it was in the wrong place.

**One home for the taking.** `Shot` held the lens and what it was set
to (make, model, focal length, f-number, focus distance) while the
ISO sat one level up as `RawFrame.iso`, with `Picture.iso` beside
`Picture.shot` saying the same thing for a JPEG. Two homes for one
answer, and the shutter in neither. `iso` and `exposure_time` (f32
seconds) now join `Shot`, and the duplicates are gone: `frame.iso`
becomes `frame.shot.iso` at its five readers, and seven `RawFrame`
literals lose a line. `decode::Probe` keeps its own `iso` and
`exposure_time`, since a probe answers from the metadata without
decoding samples and is not a frame. The doc on `RawFrame.shot` now
says what the engine's relationship to it is — it reads none of it,
and still measures the noise it acts on from the frame rather than
from the ISO tag.

**Both paths already met.** `shot_of` builds a `Shot` from rawler's
`RawMetadata`, and `picture.rs` was already calling it for a JPEG,
PNG or TIFF, so filling the two new fields fills them for every file
the editor opens. The shutter goes through the same rational helper
the aperture uses, which already drops a zero denominator and a
non-positive value.

**Three tags for one ISO.** `ISOSpeedRatings` is sixteen bits, so a
camera past 65534 writes 65535 there and means it somewhere else;
`ISOSpeed` is the thirty-two-bit tag that replaced it, and when
`SensitivityType` is 2 — which the Canon, the Sony and both Nikon
samples all say — what was recorded is the exposure index, in
`RecommendedExposureIndex`. `iso_of` (one helper now, where the
expression had been copied three times) tries them in that order,
with the saturated value and a zero in any of them treated as the
camera saying nothing rather than as an answer. The first cut had
the `> 0` filter only at the end, so a zero in the first tag
poisoned a good value in the second; the test walks all eight
combinations.

**The formatting is in core, not the editor.** `Shot::summary` and
the helpers under it (`camera_name`, `shutter_text`, `aperture_text`,
`focal_text`) are pure functions on the data with their own tests,
and the CLI's `info` and `lenses` call the same four, so the panel
and the terminal cannot drift on what 1/250 s, f/5.6 or a Nikon's
name looks like. `info` used to print `{make} {model}` and format
the aperture `{:.1}`, which said "NIKON CORPORATION NIKON Z6_3" and
"f/2.0" where the panel said "Nikon Z 6 3" and "f/2". This is not a
tone-mapping or an edit-schema decision entering core; it is how a
`Shot` reads out loud, and it belongs with the struct.

**The shutter's fraction has to be honest.** Under a second it is
`1/N`, over it `N s`, which is how a camera and a photographer both
say it. The wrinkle is the third of a second: a camera records it as
10/30 and `1/round(1/t)` gives 1/3, but a value that came through an
APEX conversion arrives as 0.3, and 1/3 is 0.333 — 11% out, a sixth
of a stop. 0.6 s is worse: the nearest fraction is 1/2, which is a
quarter of a stop and simply not what the camera did. So the
fraction is used only when it lands within 5% of the recorded time,
and anything else reads in tenths — "0.6 s", the way the body's own
display shows it. Every ordinary speed (1/8000 to 1/2) is exact and
takes the fraction. Both branches print through the same `number`,
so 0.98 s counts up to "1 s" rather than sitting at "1.0 s" next to
the "1 s" a whole second gives.

**And the aperture lands on its mark.** The same APEX conversion
gives an f-number of 5.657 or 11.314 where the barrel, the camera
and every photographer say f/5.6 and f/11: the marks are roundings
of the exact `2^(k/6)`, and the rounding at f/11 is 2.8% of it. So a
value within 3% of one of the 37 third-stop marks is printed as that
mark and anything else as itself — f/6 stays f/6, since it is 7%
from f/5.6 and 5% from f/6.3. The marks are 12% apart, so no value
can fall in two windows.

**The camera's name, said once.** A model usually repeats its maker
("Canon" + "Canon EOS R5") and a maker often says more than the body
does ("NIKON CORPORATION" + "NIKON D850"), so when make and model
start with the same word the model speaks for both, and otherwise
they are joined. The lens's maker is dropped: it is almost always
the body's, and a third party's name is already inside the lens's
own ("SIGMA 50mm F1.4 DG DN | Art"). What reaches the panel for a
CR3 is "Canon EOS R5 Mark II · RF 50mm F1.2L USM", rawler's cleaned
names rather than the raw tags — the CLI's `lens` line has always
printed the same, and the two now agree by construction.

**Two lines, and they hold their place.** The whole of it — body,
lens, focal length, aperture, shutter, ISO — is 60-odd characters
and the panel is 320px wide, so `ShotSummary` comes back in two
parts, the body and its lens, then the exposure triangle. The file
name and the two lines are one block now, 2px apart inside it where
the panel's sections are 8px, so the three read as a caption rather
than as three sections. Both lines keep their 16px whether or not
the file filled them: made conditional they would have taken 34px
out of the panel every time a picture without tags was picked, and
everything below would have jumped — the same reason the old
selection code keeps the last picture's panel until the new one's
arrives. They are cleared when a file is picked so the last
picture's numbers never sit under this one's name while it decodes.

**The export already carried it.** `RawMetadata::write_exif_tags`
copies the source's EXIF wholesale, so `ExposureTime`, `FNumber` and
`ISOSpeedRatings` were in every JPEG, PNG and TIFF the engine
writes; the export test now asserts the shutter as well as the ISO
and aperture, so a future narrowing of what is copied fails loudly.
Checked against `exiv2 -pa` on five bodies (Canon R5 II, Sony A7 IV,
Nikon Z6 III, Fujifilm GFX100S II, Panasonic S5 II) and on a
greycard JPEG: the panel, `info` and the file agree.

**Not done.** The capture date and the GPS fix are in the metadata
and not in the line; they answer a different question and the panel
has no room for them. The filmstrip and the export sheet still show
the file name alone.
