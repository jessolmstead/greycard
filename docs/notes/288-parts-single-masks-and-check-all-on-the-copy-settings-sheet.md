# §288. Parts of a section, single masks, and check all on the copy settings sheet (2026-10-10)

Roadmap 0.6.0, in the user's words: "There's no check/uncheck all.
Detail should be a heading with sub-options for texture, clarity, and
dehaze. Same for other sections. Also what are Adjustments? And Masks
should be an option including individual masks to copy over."

**What a part is.** A `preset::Part` is one slider, or the few settings
that make one control in the panel, to carry alone while the rest of
its section stays as the frame has it. We chose them section by
section:

- Light: Exposure, Contrast, Highlights, Shadows, Whites, Blacks.
- Color: Vibrance, Saturation.
- Curves: Parametric, RGB, Red, Green, Blue, R/G, B/Y, each curve whole
  (the parametric curve's four sliders and splits are one control).
- Color mixer: the eight bands, each band's hue, saturation and
  luminance together. A band is what one copies between frames (the
  blues of a sky); hue, saturation and luminance across all bands, as
  Lightroom groups them, would split one band's fix in three.
- Black and white: Weights (the eight bands into the grey), Strength.
- Color grading: Shadows, Mid-tones, Highlights (each wheel's hue and
  saturation), Balance.
- Lens: Profile (the profile's switch and its distortion), Vignetting
  (the profile's), Chromatic aberration (the profile's switch and the
  red and blue set by hand), Manual distortion, Scale (auto and the
  value), Defringe (its switch and its windows).
- Detail: Texture, Clarity, Dehaze.
- Vignette: Amount, Midpoint, Feather, Roundness.
- Grain: Amount, Size, Type.

White balance, Tint, Look, Noise, Sharpen, Demosaic and Camera profile
have no parts. Each is one choice: a white is as shot or a temperature
with its tint; a tint is a hue with its amount; a look is a table with
its strength; Noise is the profiled pass and its strength; Sharpen's
radius, iterations and threshold are one deconvolution's and mean
nothing apart; Demosaic and the camera profile are one pick each.

A section's own switch is not a part. It goes only with the whole
section. A part laid over a frame whose section is off is there, and
shows once the section is turned on; turning it on for the user would
also wake the frame's other sliders of that section, which nobody
asked for. `Part::copy` and `Part::same` sit beside `Section::copy`
and `Section::same`, and a test holds that a section's parts laid
together are the section, its switch aside.

**The file.** A preset (and the preset a sync or a paste builds and
never writes) has `sections`, whole as before, and two new keys:
`parts`, the parts of sections not carried whole, written as
"detail.dehaze" (section, dot, part), and `masks`, "join" when the
masks it carries join a frame's. Both are left out of the file at
their defaults (no parts, "replace"), so a preset of whole sections is
written with the same keys as before, the sections in the panel's
order (Look now first, so a preset carrying Look and an earlier
section lists them in a new order; a golden-string test holds the
rest), and a file without them reads as whole sections whose masks
replace, which is what every older file meant. "join" is written, and
read, only when the masks go: chosen with no mask ticked it says
nothing and is dropped. A part of a section also carried whole is
dropped on read, the whole carrying it already. A part name this build
does not know (one from a newer build) is passed over with a line in
the log rather than the whole preset refused, and a null list of parts
is none. The masks carried are simply the edit's
masks: a preset with one mask chosen holds that mask and no other, so
which masks were chosen needs no key of its own. `Preset::from_choice`
takes a `preset::Choice` (sections, parts, mask ids, replace or join);
`Preset::from_edit` is that with whole sections, so every older caller
reads as before. The CLI's `greycard presets` lists parts after the
sections, as "Detail: Dehaze", and the editor's import says "imported
Haze (1 part)".

An older build reads the new keys as unknown fields and passes over
them, since a preset is read leniently (`serde(default)`, no refusal of
unknown fields), with three results: a preset of parts alone is listed
and does nothing when clicked; a preset of sections and parts lays the
sections and silently drops the parts; and a preset whose masks join
is read as replacing, so it takes away the frame's own masks (one step
in the history, undone as any step is). We looked for a cheap way to
make an older build refuse such a file instead. The only thing an
older build checks is the edit's schema version, which belongs to the
edit and moves with every sidecar; raising it for a preset would mean
a schema step no edit needs. The other way is a name an older build
cannot read inside `sections` (a part's dotted name there, or a marker
for joining), which an older build refuses as an unknown section; but
that would make every later reader carry the special case for good,
to guard builds from before this one. We chose neither: presets move
between machines by hand, and the harm is a step that undoes.

**Masks: replace or join.** "Adjustments" is called Masks wherever the
user sees it: the sheets, the history row for a change across masks,
the guide. The enum keeps its name. Under Masks the sheet lists the
frame's masks by the names the Masks tab shows ("Mask 3" for one with
no name), each with its own check, and a choice: Replace masks (the
default, what ticking the masks always did) takes the frame's own away
and lays the chosen ones with ids 1 up; Add to masks keeps the frame's
and puts the chosen ones after them, each with an id after the frame's
highest. Joining is idempotent: a mask the frame already has, the same
in everything but its id, is not added twice, so a second paste
records nothing. On a frame with no masks the Masks heading is itself
the one check, with nothing under it but the choice; with Replace it
clears the targets' masks, as before. A mask keeps its
normalized placement on a frame of another size, and a learned mask
(Sky, Subject, People, a part) carries its ask and is found again on
each frame, as a sync and a paste already did: nothing about the
mask's content changes on the way.

**The sheet.** `crate::carry` holds the checks and is pure: a section
with parts has one check a part, the masks one a mask, any other
section one. The heading's check is on when all its parts are, off
when none is, and shows a dash between; a click on it turns the whole
section on, or off when it was all on. Check all and Uncheck all set
every section, part and mask. What the checks choose: a section all on
goes whole, exactly as before parts, so a sheet left at its sections
does and writes what it always did; a section partly on sends its
parts; the masks partly on send Masks with the chosen ids. Rust lays
the list out, two columns of whole sections in the panel's order,
split where the taller column is shortest, a section's parts two to a
line under its heading, in a scroll view the sheet sizes to the window
less its words and buttons. The rows' model is kept between clicks so
a check turning over updates its own row. The sync and paste sheet and
the preset sheet share the component (`carry-list.slint`, replacing
`section-toggles.slint`).

Each check takes the keys as the switches it replaces did: Tab to it,
Space or Return to turn it over, the focus ring on its box. A heading
partly on reads to a screen reader as checked, described as partly
checked. The sync sheet's card takes Escape itself, as the preset
sheet's does, since a key pressed on a focused check goes up through
the sheet and never reaches the window's own. The list is never given
less than four lines, however short the window.

The sync's sheet opens on its defaults (all but the masks) as before.
A paste's opens on the last sync's or paste's checks this session; the
masks are another frame's by then, so all on and all off stay so and
a partial choice ticks the masks of the same name. The preset sheet
opens, as before, on what the edit changed of the sections a preset
carries by default, and now more finely: of a section with parts, the
parts the edit changed, or the whole section when only its switch is
changed. A preset saved from the open sheet holds what was done and no
neighboring slider at zero.

Look is first in `Section::ALL`, as it is first in the panel.

**Tests.** In `greycard-edit`: a part copied alone changes only that
slider (Light's Exposure, Detail's Dehaze with the section's switch
left, a single curve); a part of a section chosen whole is the
whole's; every part moves only itself and a section's parts together
are the section, and each laid alone moves no other part; a preset with
parts and joining masks round-trips; an old file (sections only) loads
and acts as before; a whole-section preset's JSON is, string for
string, what the struct before parts wrote; an unknown part and a null
list are passed over; "join" with no masks carried is not kept; one mask
chosen replaces or joins, with fresh ids after the frame's, and a
second join adds nothing; the words ("2 sections and 4 parts"). In
`carry`: sections all on are whole as before; a part unchecked sends
the rest of its section as parts; the heading's click; Check all and
Uncheck all reach every part and mask; masks checked one by one, and
carried over to another frame's masks by name; the layout (every row
once, no two on one spot, two columns of whole sections). Through the
window, with real clicks on the checks and buttons found by their
accessible labels and asserted inside the window: a sync with Check
all, Uncheck all, Light's Exposure, Detail's Dehaze and one mask added
to the target's, the target's Contrast, Shadows, Texture and own mask
kept; a paste of one Sky mask replacing the target's, carried as its
ask, and the next paste's sheet remembering it; the preset sheet
opening on the changed parts and saving Detail's Dehaze and one mask;
a check turned over by Space and Return, Tab moving on, and Escape
from a focused check leaving the sheet; an imported preset of parts
naming its parts. The existing sync, paste and preset tests pass
unchanged in what they assert, through the new rows.

**Seen.** Captures under the editor on a headless weston, every XDG
directory under the worktree, hard links of three sample raws: the
preset sheet over a frame with a preset of parts and three masks laid
on (Light, Color, Detail and Vignette partly checked, the masks listed
under Masks with Replace and Add), and the sync sheet over the same
frame with two others selected.
