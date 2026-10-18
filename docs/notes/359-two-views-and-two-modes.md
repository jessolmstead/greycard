# §359. Two views and two modes (2026-10-18)

The top left of the window read Library | Develop, and culling was
a mode under C that could be on in either. A tester asked for a
Library tab and three for faster culling, and the question we put
to ourselves was whether that bar should be three, Library, Cull,
Develop. Working it through: the grid has no mode of its own, since
its thumbnails are the cache's either way, so Library is a view and
not a mode; culling and developing are two ways of looking at one
picture, so they are modes of the Loupe. Two views, Library and
Loupe, and in the Loupe two modes, Cull and Develop: three states,
not four. The Library name and its icon stay. We built it as a
branch to try before naming it, and after a day in it the user kept
it.

## What the user sees

Library | Loupe at the top left. In the Loupe the panel's tab row
reads Cull, in a small box of its own, then Develop, Crop, Masks and
Retouch in theirs, so it reads as two modes rather than five tabs;
the pressed entry says the mode. Inside Cull the CULLING section has
a Picture switch, Camera JPEG or Developed: the developed one is the
develop path with the panel on the Cull tab, the culling keys, Move
on and culling's undo, and none of the presets, snapshots or
history in the left pane. Compare is the camera JPEG's. The lightbox
is a view beside the two; Rejects is a folder. The left pane belongs
to the window, not a mode.

G goes between Library and the Loupe, and back into the Loupe in the
mode it was left in, so G, G lands where you were. E goes to the
Loupe. C goes between Cull and Develop in the Loupe and into Cull
from Library. D goes to Develop from anywhere, Cull, Library or the
lightbox, on the tab that was active before Cull; we considered
making D land on the Develop tab itself and kept the remembered tab,
since the tab row already keeps its tab across frames and D names
the mode, not the tab. Enter and Esc leave Cull as before. The mode
and the Picture switch are remembered across runs; the view is not.

## Inside

One new flag on the culling owner says which mode the Loupe comes
back in, set by every way into Cull and out of it, so a control
reached in either Cull still means Develop: a slider, a paste, a
preset, the look list, the retouch tools, or a view change from the
panel, none of which exist in Cull but all of which leave it. A
second flag is the Picture switch; a third holds the developed
variant and the tab to give back, never set at once with the camera
one. Library over a Loupe in Cull keeps the cull state alive
underneath. The shared segmented control gained a `picks` property,
false for the Cull box, so a click does not set the current entry
itself and Rust decides whether Cull is entered; a click on the lit
Cull box, or G then C, is nothing, so a compare put up by Compare
versions or Compare stack survives it. The Cull box can be pressed
from a screen reader.

A run the command line owns, a capture or a timing run, takes no
remembered mode and writes none, under one test that the delete
guard also reads, so a session that ended in Cull cannot make the
next timing run open in Cull and wait for a develop that never
comes. `--cull-developed` refuses the camera-Cull flags.

## Left

The capture-shaping flags used without a capture (`--tool`,
`--sheet`, `--menu`, `--press`, `--keys`, `--part`, `--tab`) still
take the remembered mode; they are repro flags for us. Going from
Developed to camera Cull writes the settings twice.

## Tests

The views and the modes: Library shows the grid and the pane and the
Loupe one picture; the Loupe comes back from Library in the mode it
was left in, by the Loupe button, Return, Esc, a double-click on a
cell and G; G, E, C and D switch and carry the selection; D goes
straight to Develop from Cull, Library and the lightbox, does
nothing under the keys sheet and is typed into a focused field; the
Cull and Develop switch changes the picture and the panel; the
Developed switch shows the developed picture with no edit panel; a
rating in the Loupe's Cull moves on; an edit in Developed Cull
leaves to Develop and undoes the edit, not the rating; a compare
holds through Cull asked again; C leaves Cull with no frame open;
the mode is kept across a settings round trip; a timing run or a
capture opens in no remembered mode; `--cull-developed` refused
beside the camera Cull's flags; every key the handler binds is on
the keys sheet. The four layout tests are byte for byte as they
were, the 28 px row being gone.
