# §283. A mask's white balance keeps its light while off (2026-10-10)

A mask's own white balance (§241) went back to the global's white every
time its switch went on: switched on, it started where the global
stood, whatever the mask had said before. So the switch could not
compare the two, which is what a switch is for, and every other
switched-off section in the editor keeps its values. Now it keeps them
too.

## The rule

The switch and the light are each kept. Switched off, the mask's
temperature and tint stay where they were; the picture under the mask
is the global's. Switched on, the same light comes back. A mask that
has never had one (new, or reset) still starts at the global's white,
As shot read as the kelvin and Duv it means, so nothing jumps.

We added no button. The slider reset gesture, a double-click on the
mask's Temperature or Tint while the switch is on, forgets the light:
both, since they are one setting, the switch goes off, the mask follows
the global again, and the sliders show the global's white, where the
next switch on starts. The sliders are disabled while the switch is
off, so a kept light is forgotten from the on state; we may accept the
gesture while off later if the extra switch proves a nuisance. The
Develop tab's sliders reset as before, to their defaults. The slider
learned `owner-resets`: a slider whose default is not a number of its
own hands the double-click to its owner (`reset`) and leaves its value
alone, so the sliders do not jump to 5500 K on the way to the global's
white.

## The format

`LocalWhite` has three states: `Absolute` (on), `Kept` (off, the light
kept) and `Off` (off, nothing kept, following the global). Three rather
than a flag beside the old two, so a switch on with no light cannot be
written. A sidecar writes the mode and, whenever there is a light, the
temperature and tint beside it:

    {"mode":"absolute","temperature":3300.0,"tint":-0.004}
    {"mode":"off","temperature":3300.0,"tint":-0.004}

`Off` with nothing kept is left out, as it always was. A sidecar from
before has no light beside `off` (or no field at all) and comes back off
with nothing kept, so its next switch on starts at the global, as today.
A build from before reads the kept form as off, as it read every `off`
(its schema ignores what comes beside a unit mode); it drops the light
on its next save, which costs the comparison and nothing on the picture.
A mode this build does not know reads as off with nothing kept, whatever
values come with it, since they are not a light this build knows how to
bring back. No `VERSION` bump: a defaulted field and a mode already
written.

The panel holds the state in `local-wb-on` and a new `local-wb-kept`;
`show_edit` sets both from the look, and the read-back makes the look
from both, so the overlay's baseline (§277) reads a kept light back the
way it reads one on.

## The develop key

The kept light reaches no pixel, so the develop key (§278) does not see
it: the key's copy of the edit turns a kept light into `Off`, beside the
adjustment names it already blanks (`LocalWhite::acting`). Switching
off changes the key (the picture goes back to the global's white);
switching on again gives back the key it had; two frames switched off
at the same global share one key whatever light each keeps. Other
switched-off sections (a mixer or a grading off) still hash their
values; this is the only one blanked, because it is the only one whose
off state has a second form with nothing kept that the same picture
can be written as.

## History

The switch is a step, as it was: "Lamp: White balance off" and "Lamp:
White balance on". The reset is one step, "Lamp: White balance reset".
History rows are named from the two edits they sit between, so an old
sidecar's step from a light to plain off, which used to read "off", now
reads "reset"; that is what the step did.

## Tests

The kept form written beside `off`, read back, through an edit's JSON
and back equal; off with nothing kept left out of the look; a sidecar
without the field reading as off; a build from before (its schema in
the test) reading the kept form as off; a later mode with values
reading as off with nothing kept. A kept light makes no matrix, and the
headless export with a kept 2800 K over everything is the global's
picture to the last bit. The key: on changes it, kept goes back to the
global's, a different kept light gives the same key, and on again gives
back the on key. The history rows for off, on and reset. On the panel:
a mask set to 3300 K, switched off (stored as kept, the sliders still
at 3300 K, one step), on (3300 K back, one step), off again and through
the sidecar's JSON and `show_edit` into a fresh panel (kept, and on
brings it back); the reset (off, nothing kept, one step, the sliders at
As shot's kelvin) and on again starting at a custom global. The overlay
test from a foreign sidecar now runs with the light on and kept, and the
white balance's switch, a look control, hides the overlay.
