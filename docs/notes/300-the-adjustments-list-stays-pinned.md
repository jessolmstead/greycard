# §300. The Adjustments list stays pinned above the Masks tab (2026-10-11)

Issue #18: with a mask chosen and a slider far down the Masks tab in
hand (the grading, the tint), going back to Global or to another mask
meant scrolling up past every section to the list. We first thought
of a Done button at the foot of the tab; we pin the list instead, as
the tab bar is pinned, so the way out is in sight from anywhere and
no new control is needed.

The Masks tab now starts with a fixed header between the tab bar and
the panel's scroll: the ADJUSTMENTS list (Global, each mask with its
switch, the chosen one with its bin) and New mask. The sections
scroll under it. What used to follow the list in the same section,
the chosen mask's name, switches and shapes, is now a section of its
own, MASK, headed with the mask's name and folded on its own; it
shows only while a mask is chosen, as the look sections do.

The list holds at most six rows, and fewer when the panel is short.
The count is read from the room the panel has under its head (the
scope, whose height depends on which scope is chosen: 84 px for RGB,
144 for Wave and Parade, the panel's width for Vector), its tab bar
and over its footer, none of which depends on the list. From that
room we take the list's own chrome (its header, rule, border and New
mask: 79 px, written out as a sum in the code) and a view of at least
150 px for the sections, and show as many rows as fit what is left,
from one to six. So the list gives up rows before the sections' view
and the footer give up room. With the strip shown, RGB has six rows
from about 870 px tall, Wave from about 930, and Vector, whose scope
is far taller, from about 1070. The count is set by
handlers, not bound: the window's height answers to what is laid
out in it, this list among it, so a binding would be a loop.

On the Masks tab the sections' view may shrink to nothing (std's
ScrollView floor is 50 px; Develop keeps it). Even so, one row and
New mask are more than the panel has under a tall scope in a short
window: with the strip shown, below about 780 px under Vector and
below about 630 px under Wave. There the footer is still pushed down
by the difference. We keep a one-row floor rather than hide every
row: a list that cannot show Global is no way back to it. What is
left over, with ten masks and the strip shown, against Develop's
footer: Wave at 600 px, 28 px; Vector from 600 to 700 px, 59 px, at
740 px 32 px, at 760 px 12 px. Develop itself overflows under Vector
below about 740 px, so the Masks tab is the first to only between
about 740 and 780 px, by at most about 12 px.

Past its rows the list scrolls inside itself. The chosen row is kept in sight on every
change that could hide it: a change of target (New mask's row at the
foot, Global back at the head), a change of the rows under the same
target (a delete, an undo, another photo), a change of the list's
height, and when the list is built again on coming back to the Masks
tab. Folded, the list is its header alone and the header names the
chosen row, so the panel still says what it edits.

The panel's scroll position is the inner scroll view's own offset, so
it counts from the foot of the pinned header: the header's height,
whether it grows with a mask, shrinks with a delete or folds, never
moves what the sections show, and the reset to the top on a tab
change is unchanged. On the Masks tab the `--panel-scroll` snapshot
offset now counts from below the pinned list too, so an offset chosen
before this change lands the list's height further down the sections.

The test helper that scrolls a control into view now takes the
pinned list's foot (or, folded, its header's) as the top of the view
on the Masks tab, so a control behind the list is not taken as in
view, and steps by less than that view's height, so a control cannot
fall between two steps.
