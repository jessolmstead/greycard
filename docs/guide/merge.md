# Merging frames

Several frames of one scene can be merged into one: a bracket into an
HDR, a turn of the camera into a panorama, or a run of focus distances
into a focus stack. Each merge is a DNG you edit like any other frame.

## Merge to HDR

A bracket, the same picture at several exposures, on a tripod or by
hand, can be merged into one frame that holds the shadows of the
brightest frame and the highlights of the darkest.

- Select the bracket's frames, two or more, of one camera and one
  size, and choose **Merge to HDR** on the frame menu. The status line
  says each frame as it is read; a 45-megapixel frame takes a few
  seconds.
- The merge is a DNG beside the first frame, named after it:
  `IMG_0001-HDR.dng`, or `IMG_0001-HDR-1.dng` when that name is taken.
  When the frames share one folder and you are not in a collection, it
  goes on top of a stack with them.
- The file is a compact float DNG: about 170 MB for three 45-megapixel
  frames.
- It opens looking as the middle exposure did, with that frame's edit
  (its exposure left at zero), and the highlights the middle frame
  lost are there to bring down with **Highlights** or the exposure.
  **Highlights** at −100 brings the brightest of them just under
  white.
- A frame that moved between shots, by a hand's shift, turn, or
  lean-in, is put back in place, and the log says how far. When one
  did, whatever moved in the scene itself (a person, a branch, a
  wave) is taken from the middle frame alone rather than doubled. A
  near scene shot by hand still shows parallax no merge removes.
- Asking for a second merge while one runs starts it when the first is
  done, and the status line says so.
- The file is never seen half-written: it is written under a hidden
  name and renamed when done. If a merge is cut short, its hidden part
  file is removed the next time you merge to that name.

From the command line: `greycard merge hdr IMG_0001.CR3 IMG_0002.CR3
IMG_0003.CR3`, with `--reference` to pick the frame the others are put
on (counting from 0), `--out` to name the file, and `--on-exists` for
when that name is taken.

## Merge to Panorama

Frames shot turning the camera, each overlapping the next, can be
stitched into one wide frame.

- Select the frames, two or more, of one camera, one size, and one
  focal length, in the order you shot them, and choose **Merge to
  Panorama** on the frame menu. The status line says each frame as it
  is read, then as it is blended in.
- The frames' lens is corrected first, from its profile, as the
  develop would correct it, and the develop does not correct the
  panorama again. With no profile for the lens, or the Lens section
  off, the frames are stitched uncorrected, and the log says so.
- Up to about 90 degrees across, the frames are laid flat; wider, on a
  cylinder, which holds up to 120 degrees across the turn. A camera
  whose focal length or sensor size is not known is always laid flat,
  and the log says so.
- The frames' brightness and color are matched where they overlap, and
  each seam is led around what differs between them, so a person who
  moved, or something close to the lens, comes from one frame rather
  than twice. Turning the camera about the lens itself, on a tripod's
  panorama head, leaves the least to hide.
- The result is cropped to the largest rectangle the frames cover, and
  is held to 100 megapixels for now.
- The merge is a float DNG beside the first frame, named after it:
  `IMG_0001-pano.dng`, numbered when the name is taken, and on top of a
  stack with its frames, as a merge to HDR is. It is seen from the frame
  in the middle of the turn, whatever order you shot in, and opens with
  that frame's look: its edit less its exposure, its lens corrections,
  its crop and straightening, its retouching, and its masks, which were
  placed on one frame.
- When that edit uses the learned denoiser, each frame is denoised as
  it is read.

From the command line: `greycard merge pano IMG_0001.CR3 IMG_0002.CR3
IMG_0003.CR3`, with `--reference` to pick the frame the others are
turned onto (counting from 0), `--out` to name the file, and
`--on-exists` for when that name is taken. A second merge to a file
that one is still writing is refused.

## Merge to Focus stack

Frames of one subject shot at several focus distances can be merged
into one frame that is sharp everywhere.

- Select the frames, two or more, of one camera and one size, in focus
  order, and choose **Merge to Focus stack** on the frame menu. The
  status line says each frame as it is read. Frames are read one at a
  time, so a long stack needs no more memory than a short one. If too
  little memory is free, the merge is refused: close other programs and
  try again.
- The frames are put onto the middle one, scale included, since a lens
  breathes as it focuses. A frame that cannot be lined up is left out,
  and the log says which and why.
- The merge is a DNG beside the first frame, named after it:
  `IMG_0001 stack.dng`, numbered when the name is taken, and on top of
  a stack with its frames, as a merge to HDR is. It opens with the
  middle frame's edit, its exposure kept, less the learned denoiser.
- Each frame is developed as the editor develops it, with its
  chromatic aberration corrected, before the frames are merged.
- A focus stack keeps its recipe, as a merge to HDR does, so it can be
  made again from the Noise section with the frames denoised.

From the command line: `greycard stack IMG_0001.CR3 IMG_0002.CR3
IMG_0003.CR3`, with `--reference` to pick the frame the others are put
on (`middle`, `sharpest`, or a number counting from 0),
`--selectivity` and `--max-residual` to tune the merge, `-o` to name
the file (a `.tif` writes a 16-bit linear TIFF instead),
`--uncompressed`, and `--on-exists` for when that name is taken.

## Making a merge again

A merged file keeps its recipe: its frames' names with a check of
each, the merge's options, and where every frame was placed. While the
frames are beside it, unchanged, it can be made again from them.

- In the window, open the merged file: in the Noise section the
  learned denoiser's tiers give way to a row of their own over
  **Denoise the frames and merge again**. The tiers start at the one
  the merge ran, or off when none ran. Pick one and press the button;
  the status line follows the merge as it runs, and the picture
  reloads when it is done. The row is not there when a frame is
  missing or changed, when a frame is neither Bayer nor X-Trans, or
  when sidecars are off, and it waits while another merge runs.
- `greycard merge again IMG_0001-pano.dng --denoise balanced` merges
  the frames again with the learned denoiser on each (`fast`,
  `balanced`, `best`, or `none`). The frames land where they did, to
  the pixel, so the file's edit, crop, and masks still fit; only the
  noise changes. Nothing is matched or fitted again, so it is quicker
  than the first merge.
- The file is replaced under its own name, and its edit kept; `--out`
  writes it elsewhere instead.
- A frame missing from beside the file, renamed, or changed since is
  named, and nothing is written. A file merged before recipes were
  kept is refused: merge its frames again.
