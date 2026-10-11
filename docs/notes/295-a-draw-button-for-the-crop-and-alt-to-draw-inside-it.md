# §295. A Draw button for the crop, and Alt to draw inside it (2026-10-10)

In crop mode a drag inside the crop moves it and a drag outside it draws
a fresh one. A crop at the whole frame, which is where every frame
starts, leaves no outside to press on: Crop, then a drag on the picture,
moved a crop that had nowhere to go and so did nothing. The only way to
draw was to shrink the crop first.

We added a **Draw** button beside Crop in the CROP section. It is a one
shot, like the level and the guides: pressing it puts Draw in hand
(`crop-draw-armed`, shown as the primary look the way Level and the
guide buttons show theirs), and the next press and drag anywhere on the
picture, inside the crop or out, draws a fresh crop from the press point,
held to the aspect in force. The release puts it down. A click that never
clears the drag threshold also puts it down and does nothing else: no
zoom and no change to the crop, since a click with a tool in hand is
spent on that tool. Escape puts it down, after the level and the guides
in the Escape chain, and so do leaving crop mode (a `changed` handler on
`crop-mode`, so the tab switch and culling are covered as well as Done),
taking up the level or a guide, and opening the grid. Like Level, Draw
brings crop mode with it when pressed outside it.

The draw goes through the existing path unchanged: the viewport's picture
area calls `crop-draw-pressed` once a drag has cleared four pixels, then
`crop-draw-dragged`, then `crop-draw-released`, which records the change
as a crop row in history like any other. What Draw changes is only who
receives the press. Inside or outside was never a test the code made; it
was which element sat under the pointer, since the crop's body and its
eight handles are touch areas laid over the picture's own. With Draw in
hand those handles are disabled, so the press falls through to the
picture's area, which draws in crop mode wherever the drag starts.

Alt and a drag on the crop does the same without the button. The handles
already read Alt off their pointer events, so a handle pressed with Alt
latches that, keeps the press point in the picture area's coordinates,
and calls the draw callbacks instead of the move ones, past the same
four-pixel threshold. Some Linux desktops take Alt and a drag to move the window
(Xfce, MATE, Cinnamon and Plasma 5 by default; GNOME and Plasma 6 use
the Super key and leave Alt alone; Windows and macOS pass it through),
so the editor never sees it there. The button is the way the guide documents first,
and Alt is the shortcut where the desktop leaves it free.

Escape reaches the window only once the keys are its own again: the
button clicked last holds them until the pointer comes back over the
picture, which it does on the way to drawing, and which works here
because the disabled handles let the picture's area see the hover. The
level and the guides have the same button-holds-the-keys shape but their
overlays cover the picture, so a hover does not hand the keys back for
them; that gap is on the roadmap.
