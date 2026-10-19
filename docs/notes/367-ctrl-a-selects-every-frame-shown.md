# §367. Ctrl+A selects every frame shown (2026-10-18)

The selection had Ctrl+click, Shift+click and Shift with Home and
End, and no way to take the whole list at once; a tester reached for
Ctrl+A in the grid and nothing happened. It selects every frame
shown now, the list as the filter, the sort and the closed stacks
leave it, in the strip, the grid and the lightbox alike, with the
current frame left where it is so a Shift+click after it still
measures from there. A second press changes nothing, and a text
field with the focus keeps Ctrl+A for its own text, since the window's
keys never hear what a field accepts. One line in the shared part of
the key handler covers the three places, as the grid lets the keys it
does not take fall through; Home and End need a copy of their own
there only because they set the scroll as well. The help sheet and
the guide list it with the rest.
