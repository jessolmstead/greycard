# §749. A key moved beats a key by default (2026-11-26)

Redo was the one action the Keys page could not move, because it has
two keys out of the box, Ctrl+Shift+Z and Ctrl+Y, and a binding holds
one (discussion 204).

**Alternates.** A line of the keys table may now carry alternate
default chords after its first. While the action is unmoved it holds
them all: each resolves to the action, each counts as taken for a
conflict, and its row and the keys sheet show them joined. Moved, the
action holds the one chord it was given and lets the rest go, free
for another action. Set back to its first chord, or on Reset to
defaults, it holds them all again. We did not build a way to bind
several chords by hand; one moved key was what was asked for.

**Who gets a press.** 0.12 let any action be put on Ctrl+Y, and the
press went to it, since Redo's line was fixed and actions were asked
first. With Redo an action, the list's order would have handed that
press back to Redo on an upgrade. So the order is now by intent: an
action the user moved onto a chord, then a moved step's Shift form,
then an action holding it by default, then an unmoved Shift form,
and the list's order only within one of these. This holds for every
action, not Redo alone: Crop moved onto Ctrl+Z crops. Both rows still
say the conflict in red.
