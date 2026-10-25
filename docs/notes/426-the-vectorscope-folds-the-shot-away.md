# §426. The vectorscope folds the shot away

*2026-10-25*

Testers saw the panel jump when switching scopes: the histogram is 84
px, the waveform and parade 144, and the vectorscope was square at the
panel's width, about 300. The scopes are pinned over the sections, so
every switch moved all of them.

We tried every scope at one 180 px height first. It holds the panel
still, but the histogram, the default, grows 96 px: at the 800 px
window the editor asks for, the Masks tab's shape buttons fell below
the view and the window's least height rose from 509 to 605, above
what a 1280 by 720 screen gives it. Not taken.

So the histogram and the waveforms keep their heights, and the
vectorscope's circle comes down to 180 px, centered in its box. The
marks are placed as fractions of the picture they sit on, so they live
with it in that square, and the graticule and the skin-tone line are
drawn into the picture and scale with it. Nothing on the vectorscope
takes a click, so there was no hit-testing to move. While it shows,
the shot's lines under the file's name fold away (the name and the
undo buttons stay), and come back when another scope is chosen. They
are folded by the scope alone, not by a fold of the user's, so there
is no state of theirs to keep.

The two heights do not cancel exactly. With the usual three lines
folded (54 px with their spacing), the sections move 42 px down from
the histogram and 18 up from the waveform; with the time and the
companion lines too, 6 down and 54 up. Against 200 and more before,
that is the most two fixed heights allow without the circle changing
size from frame to frame. A harness test switches to the vectorscope
and back from both, checks the shift is that arithmetic, the lines go
and return, and the sections return to where they were.

## Review

A fresh review found no defects. It offered a 168 px circle, which
would split the leftover movement evenly at 30 px each way instead
of 42 down and 18 up; we keep 180, since the circle is what is being
read. The plot's width takes the smaller of the panel's two sides,
which could stretch the picture on a panel under 180 px wide; the
panel is a fixed 320 today.
