# §666. A ratio over no crop is a crop (2026-11-13)

Issue 145. Under Last used at 3:2, a 4:3 frame opened with the Crop button
and closed with Done showed 3:2 on screen and exported 4:3. Moving the box
first exported 3:2.

**Cause.** The Crop button laid the starting ratio on the panel and took it
into the edit (§347's Taken), but fitted no crop, so the sidecar held
`aspect 3:2, crop none`. Everything that frames the picture through
`Geometry::frame` (the viewport, the export sheet's size) fits a crop to the
aspect when none is stored. `Geometry::is_identity` ignored the aspect, so
every path that skipped the geometry on identity skipped it: the export, the
kept pictures, and `framed_size`. Picking from the list or dragging stored a
crop, which is why it was "sometimes".

**Fix.**
- `open_under_start` fits the crop on Taken, as a pick from the list does
  (`refit_crop`). The edit stores the crop the box showed, one step as before.
  Pressed before the develop says the frame's size, nothing can be fitted
  yet. `crop_fit_due` holds the fit with the frame it was taken on (its
  file and version), and `start_landed` makes it when that frame's develop
  lands, Done pressed or not, unless the user has set a crop since. A
  develop of any other frame lets it go, however the panel got there
  (the strip, the grid, or culling, which changes the frame without the
  browser's open). The save is rescheduled, so it is one step when the
  develop lands inside the save's wait, and a second when it lands after;
  both read "Crop", as the Crop button's one step does.
- Held starts (Original, a frame arrived at with the crop open) and Free
  store nothing, as before.
- `is_identity` became `is_identity_on(w, h)`. With no crop and an aspect
  that may frame (`frames_by_aspect`: a ratio, or Original stood on end by
  the Portrait switch), the geometry is the identity only where `frame` is
  the whole source, to the half pixel. So 3:2 over 4:3 cuts, and 3:2 over 3:2
  is the whole frame and exports to the byte as no geometry. It needs the
  size, so every caller passes the one it has. The sidecars 0.11.0 wrote as
  an aspect alone now export as they display.

**Callers.**
- Export (`finish::render`, the worker's `scene`): asked at the developed
  picture's size. The picture and the masks' bands are framed together, so
  the vignette and the masks, already placed through `frame`, now land on
  the picture they were placed for. An aspect alone exports to the byte as
  its fitted crop stored.
- `geometry::apply` and `apply_plane`: asked at the image's own size.
- `framed_size`, `fitted_size`, and `edited_pictures::make`: asked at the
  full develop's size, so the size and the cut agree.
- The viewport, the export sheet's size, history, Reset, sync, and copy
  already read `frame` or compare whole geometries: unchanged.

**Kept pictures.** The develop key already included the aspect, so an
aspect-only edit's key did not change, and its kept pictures were the whole
frame. `ASPECT_FRAME` ("aspect-frame-1") is taken into the key of an edit
where `frames_by_aspect` holds, and into `keys_made_under`, so the index
settles its stored keys again and the grid, strip, and archive cells are
remade. The default edit, a stored crop, Original as the frame stands, and
tone-only edits keep their keys. A ratio equal to the frame's own shape is
keyed anew too, because the key is made without the size: its pictures are
made again, the same.

**Not done: the camera's own aspect.** Olympus and OM System bodies record
the aspect chosen in camera in the ImageProcessing makernote: `AspectRatio`
(0x1112, two bytes) and `AspectFrame` (0x1113, the chosen rectangle as x0 y0
x1 y1). rawler reads only that IFD's CropLeft/Top/Width/Height (0x0612 to
0x0615), the sensor's valid area, so Original is the sensor's shape (4:3 on
an OM-1) whatever was chosen in camera. The OM-1 files at hand were all shot
at 4:3 (`AspectFrame 0 0 5183 3887`), so what a 3:2 frame writes is unseen.
The roadmap carries it.
