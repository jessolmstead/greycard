# 183. What a culling key answers with, and moving on after it (2026-09-27)

**What a culling key answers with.** The first outside tester put it
plainly: press X in culling and nothing on the picture says so. The
strip tile's 9 px badge was the whole answer, and with Tab on there
was none at all. §117 argued against a word in the status line, and
that argument still holds — a cull is one key and the arrow to the
next frame, and the develop that arrow starts outside culling would
write over the word before it was read — but in culling nothing
develops, the eye is on the picture and not the strip, and the strip
may be put away. So the answer goes where the eye is, in two parts.

The frame's own badge, in the top right corner of the picture: the
strip tile's `ThumbBadges`, the same component at a 16 px glyph
rather than 9, so what it says and what the strip says cannot drift.
It reads off the same sidecar the strip row does (`show_frame_tags`,
called when the selection lands and whenever that frame's badges are
put out), so the two cannot disagree either. In the compare view each
tile carries its own at 14 px, in its own top right corner beside the
name at its top left; the `CompareTile` grew the three numbers the
`Thumb` already had, filled in `cull_frame` from the sidecar as the
tiles are laid. One frame alone carries the loupe's badge; two or
four carry the tiles'. The badge is a badge and not a word — a
culler reads three stars faster than "3 stars" — and stays up, since
it is the frame's state and not an event.

The word is for the event: "3 stars", "Rejected", "Pick",
"Unflagged", "Red", "No label", low in the middle of the picture,
where the eye is and where it covers the least, on a plate that
fades over a quarter second. It says what the frame carries now
rather than which key was pressed, so a label key on a frame that
already had that label — a toggle, §117 — says "No label", which is
the settled change `set_meta` now hands back. The flag's past tense
is the one a culler says aloud; the other two name a state.

The word goes down on a timer of its own, 1.2 s, and nothing else
takes it down: not the arrow, not the next frame's picture, not a
decode landing. A culler who rates and moves on in one motion sees
what the key did on the next frame, which is the point — the tester's
complaint was the arrow wiping the only answer there was. The next
key restarts the timer with its own word. Rust holds the timer
(`State::notice_timer`) and two properties, the text and whether it
is up; the text is left standing through the fade so the plate does
not shrink as it goes. The word is culling's only: outside culling
the strip is the browser and the develop replaces the picture, §117's
case, and nothing there changed.

**Move on after a key.** Lightroom's auto-advance, off by default:
the key and the arrow are two presses until a culler asks for one,
and a switch that moved the frame under a hand that did not expect
it would cost a wrong rating on the next frame. The switch is in the
CULLING section, where the keys it changes are explained, and is
kept in the settings as it is flipped (`keep`, as the Settings
sheet's own switches are), so a session that never closes cleanly
keeps it too.

On, a rating, a flag or a label key moves the selection to the next
frame of the filtered list by the arrow's own path — `step`, the
callback the right arrow invokes — so it stops at the end as the
arrow does and opens the frame as the arrow would. There is no
second path to the next frame. Two cases stay where they are. A
frame the key took out of the filtered list already hands the
selection to the nearest frame still shown (§117's rule), which is
the next frame or the one before at the end; moving on as well
would skip one. And a set of several frames rated together stays a
set: the arrow collapses the set to one frame, and a hand that
Shift-selected three to rate them wants them still selected after.

With move-on on, a quick second press lands on the next frame before
its picture may be decoded: the badge and the name have already
changed to the frame the key will go on, and the picture follows,
which is what Lightroom's auto-advance does too. The key goes where
the selection is, and the selection is what the badge and the name
say.

A held key is one press. The window's auto-repeats of a culling key
are dropped in the key handler (`e.repeat`), always and not only
with the switch on: a repeat has nothing to add to a rating, would
flicker a label on and off, and with move-on would rate a run of
frames at the keyboard's repeat rate, one undo step each. The test
sends a press and three repeats of 3 with the switch on and finds
one frame rated and one step taken.

**Move-on and undo.** The change is recorded before the move is
asked for, so the `tags` step (§179) names the frame that was rated,
not the one moved on to. Ctrl+Z then does what it already did for a
culler who rated, arrowed on and pressed it: the frame goes back to
what it was, and since it is no longer the one on screen the
selection goes to it. Redo makes the change again on that frame and
stays there — a redo is not a key, so it does not move on. A key
pressed after the undo rates the frame on screen and moves on again.
The test walks this: four frames, a key each, the tags each on the
frame the key was pressed over and no other, two undos back, a redo,
and a key after.

Undo and redo get the word as well, since a culler pressing Ctrl+Z
over a picture is owed the same answer a key gives: "Undo: No
stars", "Redo: Rejected", the step's direction and then what the
frame carries now, in the key's own words, read off the difference
between the frame's tags and the ones the step puts back
(`tags::between`). A step that moved several frames says "Undone"
or "Redone" and no more, since one word cannot name three frames'
states. A step that finds none of its frames (all gone, or all
changed since, §179) says nothing, as it did nothing.

Leaving culling takes the word down and stops its timer, or coming
back within the second would show the last key's word over
whichever frame the mode reopened on.

**`--cull-key`.** A hidden flag beside `--turn`: the culling key to
press once the first picture shows, for a snapshot of the badge and
the word. The flag is cleared from the event loop as `--turn`'s is,
and the capture waits until it has been. The snapshots for this
change were taken with it, on a folder of six sample raws, the
loupe with the panels up and with Tab on, and the compare view at
two and four.
