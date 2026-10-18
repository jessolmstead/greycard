# §347. Three things a tester did on every frame: the crop's starting aspect, As shot that lets go by itself, and a folder imported in place (2026-10-17)

Three 0.8.0 items off the roadmap's Next list, each a step a tester
repeated on every frame or every job. One commit each.

## The crop's starting aspect

A tester crops with the frame's own shape locked, as Lightroom opens
its crop, and had to pick Original from the list on every frame. The
Settings sheet gains a CROP section with one row, Aspect: Original
(the default), Last used or Free. It is kept in `settings.json` as
`crop_start`, and the aspect last picked from the list as `crop_last`
in `panel::crop::aspect_key`'s spelling ("3:2", "Custom 7:5"), both
written as they change (through the run's settings file, so a capture
or a batch run leaves them), not gathered at the close.

When the starting aspect is laid matters more than what it is. The
aspect is part of the edit, and an aspect over no crop is not free:
`Geometry::effective_crop` fits the largest crop of a ratio about the
center when there is no crop. So it is laid when the crop opens, not
when the frame does: when crop mode is entered (Crop, Draw, Level and
the two perspective guides all come through `crop-toggled`) on a
frame with no crop under Free, which is what a frame never cropped,
or reset, holds; and on a frame opened while crop mode is still on,
once its develop lands and says which way up it stands
(`crop::start_landed`, once a frame, so a Free picked there stands).
It is set as a pick from the list sets it (`pick_aspect`, so the
Portrait switch takes the frame's way up under a ratio and is off
under Original).

A first build put it in the edit there and then. The export was
pixel-identical under Original, but the frame was not: a `--tool
crop` on an uncropped raw wrote a sidecar with the aspect and no
crop, a history step, the edited pencil on the cell, and the cell
stopped showing the camera's JPEG; with crop mode on, arrowing
through a shoot did that to every frame passed. So the aspect is now
held on the panel alone (`aspect-held`): `read_edit` keeps the edit's
own aspect and Portrait flag while it is held, and the first act on
the crop takes it into the edit. Every act ends in `set_crop` with a
crop (a handle's drag, a draw, a pick or a turn of the switch, an
angle, a quarter turn), so `set_crop` is where the hold lets go;
`show_geometry`, which puts any edit on the panel, and Reset let go
of it too. While it is held the viewport draws the held aspect's box
(the panel's geometry, not the edit's), so the box matches the list,
and the drag that ends the hold starts from that box. Original over
no crop is the whole frame, so under Original there is nothing to
store until then. Under Last used at a ratio, pressing Crop does crop
the frame, which is what that setting asks for, and that is the one
case taken into the edit at once; a frame arrived at with the crop
already open holds the ratio until it is touched, so moving through a
shoot crops nothing the user has not touched. A frame that holds a
crop, or an aspect picked for it, keeps its own whatever the setting
says.

"Last used" is the aspect last picked from the list or typed in the
custom box, on any frame: `pick_aspect` now says when the aspect it
settled on differs from the one the crop was fitted under, which is a
pick, and the custom box's half-typed "16:" is still passed over. A
Reset is not a pick of Free. Nothing picked yet is Free.

What we did not do: lay the aspect on frames as they open with crop
mode off. The aspect list then shows Free on an uncropped frame until
the crop opens; the guide says when it takes hold.

## As shot turns itself off

The global Temperature and Tint sliders were greyed under As shot,
and a tester switched As shot off on every frame before touching
them. They are live now whenever the frame is a raw, showing the
camera's white as the decode read it (`develop.as_shot_white`, kept
from the open's `as_shot` so it can be shown again). Moving either
turns As shot off on the Slint side in the same change, before
`develop-changed`, so the save that follows reads one change, Custom
at the slider's value, and records one step ("White balance 7400 K"),
as a slider's move always has; the As shot switch going off is not a
step of its own. Switching As shot back on (`as-shot-toggled`) puts
the sliders back at the camera's white and develops, a step of its
own as before ("White balance as shot"). A double-click on either
slider, which reset it to 5500 K or 0, now puts that slider alone
back to the camera's value (`color::slider_reset`), leaves the other
where the user set it, and does nothing else: As shot stays off, even
when both sliders then stand at the camera's values, so the edit is
still a custom white that happens to match. Turning As shot on is the
one thing that puts both back. Each double-click is one step, as a
move is. A reset to 5500 K, with the sliders live from the start,
would have meant a white the camera never said. `show_edit`
shows the camera's white on the sliders for an As shot state too (an
undo, a history row), so the sliders never stand at a stale custom
white while As shot is on. A mask's own white balance is unchanged:
its switch on the section's title still makes its sliders live.

The proof is on the developed picture: a white patch, neutral under
As shot, warms when the Temperature handle is dragged, and is the
same pixels again with As shot back on.

## Import a folder in place

A tester asked to import a folder with its subfolders. The Import
sheet's opening choice stays Copy, unchanged: the card found by its
DCIM folder, From filled in, the count, the rename, the preset, the
verified backup, the card never written. A row at the top offers
Copy or Add in place, and the sheet opens on Copy every time
whatever it was left on.

Add in place is Add a root behind the door a Lightroom user looks
for. It takes any folder (Choose...), counts its frames by the same
scan the copy uses and the folders under it (`import::subfolders`,
walked as the scan walks: hidden ones and links left out), and Import
hands it to `roots::add_and_open`: the same look off the window's
thread and the same `add_checked` as Add a root, with one more check
kind, `AddOpen`, that opens the folder once it is in, its own root
view, or for a folder under a root already that root's branch view of
it, deep. Nothing is copied, and nothing is written beside the raws
but what opening a folder writes. A folder with no frames the editor
opens is not offered; Add a root still takes one. A folder under it
that cannot be read is passed over and counted rather than refusing
the whole folder, which Add a root would take; the walk is the
copy's too, whose note under From says the same, those frames not
imported. Copy never claims success over frames it could not reach:
the scan's count of unread folders goes into the `Report`, whose
line ends "1 folder could not be read, its frames not imported" on
the status line and the terminal, and `--import` exits 2 then, as
`--export` does for a run that left frames out (1 stays a stop on an
error); the button counts only the frames the scan reached. A card
with a bad directory must not look imported and be formatted. The
button says "Add N frames" there, since nothing is
imported, and a folder that is itself a root already opens as that
root's view.

Copy's Folders pattern gains `{folder}`: the frame's folder under
From, its levels joined by `/` (`import::folder_under`), empty for a
frame in From itself. It is the one token whose separators are kept,
so `{folder}` alone makes the source's folders again under To and
`{date}/{folder}` nests them under the day; in Name its `/` becomes
`_` as the pattern's own does. Its levels are `/` alone: a `\` is a
character of a folder's name on Linux and becomes `_`. Empty, it
takes one `-`, `_`, space, `.` or `/` beside it with it, one the
pattern wrote and never the end of a token's value (a name ending
in `_` keeps it), so
`{folder}-{name}` names a frame in From itself `{name}`, not
`-{name}`. Each level goes through `naming::safe`
as any other, so a level of dots is no level and nothing climbs out
of the destination. `--subfolder` takes it as the sheet does, the
pattern code being shared. The backup takes the same folders, as it
takes every pattern's.

`--sheet import-in-place` opens the sheet on Add in place with
`--import`'s folder read, for the captures.
