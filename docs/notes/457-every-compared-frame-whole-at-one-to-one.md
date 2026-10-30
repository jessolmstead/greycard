# §457. Every compared frame whole at 1:1

*2026-10-30.* In culling's compare view at 1:1 only the chosen frame
was sharp (issue 56). The cull cache held one full-size camera JPEG,
the chosen frame's, and the other tiles were drawn from their
screen-size previews scaled up to the zoom: soft where the whole
point of the view is sharpness. Now the full copies are a map by
cell, bounded to the frames the view shows, at most four. At 1:1 each
tile asks for its own, the chosen frame first and the rest in tile
order; a copy goes when its frame leaves the set, all of them at a
fit, and the GPU texture made from a copy goes with it. One frame
alone behaves as before. Compare versions keeps its old behavior: its
tiles are one file's edits, so only the chosen tile takes the camera's
whole JPEG and the others keep their edit pictures, rather than four
decodes of the same file. Four 45-megapixel copies are about 720 MB
in memory and the same on the card; the log says the count and the
size once per set, and nothing is capped: a compare at 1:1 is asked
for on purpose and lasts as long as the look.
