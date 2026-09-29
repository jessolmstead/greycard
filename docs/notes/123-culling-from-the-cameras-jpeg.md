# 123. Culling from the camera's JPEG (2026-09-20)

Culling from the camera's JPEG (2026-09-20)

§80 asked for a loupe that never develops, and this is it. C enters
culling mode (and leaves it); a Cull button on the grid's header does
the same. The viewport shows the frame's embedded JPEG fitted and
turned by the orientation tag (and by the edit's own quarter turns
and mirror, so it agrees with the strip), the arrows move along the
folder, the rating keys of §117 work as they do, and no develop
starts. The panel's tab is set to a "Cull" the tab bar does not
list, so every develop section goes away by its own condition and a
CULLING section takes the place: what the mode is, the filter, the
compare count, the move-rejects button and a Develop button. The
scopes are hidden (there is no develop to read), the navigator is
empty, and the status line says what is on screen and at what:
"culling: the camera JPEG, 5464 × 8192, fitted, through the monitor
profile only; Enter develops".

**The second path in the shader.** §57 chose not to add it for the
held picture, and §80 said it was earned here. The camera's JPEG is
display-referred sRGB, so it goes nowhere near the working pipeline:
`cubic.z` tells the fragment shader the source is encoded already,
and after the sample it goes straight to a display table and returns.
Its own table, not the develop's: §59's is built from the export
sheet's output space through the proof to the monitor, and the JPEG
is sRGB whatever the sheet says and a proof is a look at one export,
so the encoded path has a second table, sRGB to the monitor's profile
and nothing between, rebuilt only when the monitor changes. The
review caught the first cut reading the JPEG as Display P3 when the
sheet said so (means 168/116/85 against 161/118/90); now the loupe is
the same to the byte under sRGB and under P3. Twelve lines. The
texture is `Rgba8Unorm`, sampled as floats, so the same binding and
sampler serve; `Params` gained a `tile` origin so a view can be drawn
into a rectangle of the target with a viewport and scissor, which is
how the compare view draws two or four pictures in one frame. The
naga test validates it with the rest.

**The prefetch.** The worker is busy with the folder's thumbnails,
and a frame under the arrow cannot wait behind them, so the previews
are decoded on threads of their own (`cull::Prefetcher`, two or three
of them, a quarter of the cores, detached for the life of the process
and idle on a condition variable when nothing is wanted). The window
is a dozen either side (`REACH`), decoded outward from the selection
with the direction of travel first at each distance, so the frame
the next arrow lands on is the next one made. Each is box-downscaled
by the nearest whole factor to the view's long edge as it was when
the mode was entered (1410 px here: 910 × 1365 of a 45 MP portrait,
1500 × 1000 of a 24 MP landscape; a window resized afterwards keeps
that size until the mode is left and entered again), turned, and kept
as RGBA bytes, five or six megabytes each. The bound is a byte budget
rather than the count: `BUDGET_BYTES` is 256 MB, and the reach is
what that holds at three bytes a square pixel of the long edge, so at
1410 px the full dozen either side (25 frames, 130 MB at most; 86 MB
measured at sixteen held) and at a 2560 px view five either side,
where the dozen would have been 440 MB. The wanted list is replaced
whole on every move, less what is cached, and the prefetcher leaves
out what a thread is on. The slot a preview goes in is the request's
(a full-size request fills the one full-size slot, a screen-size one
is a window preview) and never the picture's: a JPEG no larger than
the view comes back whole from a screen-size request, and the first
cut, which took "whole" for "full-size", put it in the full slot,
dropped it at a fit, and re-queued it for ever, so the Sony ARW and
the Panasonic never showed.
The first cut kept an "asked" set on the UI side and dropped a decode
that was still queued when the list was replaced, which then never
came; the second sends the whole list every time and the threads own
the in-flight record. One race is left and closed by hand: a thread
forgets a decode a moment before the delivery reaches the UI thread,
and the 1:1 request, which is made every frame, would ask again in
that moment, so the mode remembers the one full-size copy it asked
for.

**Measured**, release, the 16-core desktop, on the 21 CR3s of the
sample folder (`--time-cull 20`, a step every tenth of a second as a
hand arrows, the time from the key to the frame that shows the
picture): mean 7.1 ms, min 1.2, max 35 over the 20 steps; the mode's
first picture 129 to 165 ms from nothing. The 35 is a step that ran
ahead of the window, which then costs one decode; at 16 ms a frame
the rest are one frame or under, which is what §80 asked. The decodes
themselves (`previews_of_the_samples`): 24 MP R6 II frames 35 to 61
ms, the R5 57, the 45 MP R5 II 69 to 108 (148 under load), the Nikons
42 to 48, the X-series RAFs (a 4000 × 3000 preview) 35 to 53, the
Sony ARW's 1616 × 1080 preview 11, a phone DNG's 1024 × 683 in 6.

**1:1** asks for the JPEG at its own size (`Want { size: 0 }`) ahead
of everything else and shows the screen-size copy magnified until it
lands, the status saying so; 365 ms for the R5 II's 8192 × 5464, most
of it the RGBA conversion and the turn on one thread, and a 179 MB
texture. The copy is dropped at a fit. The view's zoom and center are
kept in the JPEG's own pixels whichever copy is on the GPU, so 1:1
means 1:1 and the swap from the small copy to the full one does not
move the picture. A small embedded preview (the ARW's, the DNG's;
under 3000 on the long edge) is shown at its own pixels and called
"a small camera preview, 1080 × 1616" rather than blown up.

**Compare.** V cycles one, two, four; the panel's row does the same.
The set is a run of rows with an anchor that slides the least that
keeps the selection in it (`anchor_for`, tested), each frame fitted
to its tile, or at the zoom with the selection's center as a fraction
of each frame, so a 1:1 of four shows the same corner of all four.
A set of four at the end of a folder of three is three tiles in the
square's grid and an empty cell (the first cut laid three across a
two-by-one grid and wgpu refused the scissor). A rule round the
selection and each frame's name are Slint over the texture, for two
or more only; a click on a tile selects it.

Tests: the window's bookkeeping as the index moves, the decode order,
the byte budget, the slot from the request, the filter and the
nearest row, the compare anchor and the tiles for one to four, the
box downscale (a factor past the short side held) and the turn
against `develop::orient`, the rejects move on a temp directory (a
frame with a sidecar, one without, a raw's name taken, a sidecar's
name taken, nothing rejected), the control overlay, the keys (C, V,
Return, Escape, the sheet, Ctrl+C left alone), and the shaders
through naga.

**The filter** is the browser's, not the mode's: All, Picks, No
rejects, on the CULLING section and the grid's header, and
`--filter` on the command line. The window's `selected` became a
row of the filtered list and `current` stayed a file; `shown` maps
one to the other, and the strip's and grid's ranges, the thumbnail
deliveries, the badges and the arrow keys go through it. A frame
that leaves the list under a key (X under No rejects) hands the
selection to the nearest frame still shown, which in the mode is a
switch and outside it a develop, as Lightroom does it.

**Leaving.** Enter, Escape, the Develop button, a develop tab, or any
develop or look control reached (the section switches, undo, the
history, a snapshot, a preset: every path to a develop checks the
mode first). The frame's edit goes on the panel and its real develop
is asked for; the camera's picture is held (`cull_hold`, drawn by the
same path) until that lands, when the viewport swaps to ours in one
frame. They differ by design; the swap is the point. On the 45 MP
frame: opened in 0.17 s, developed in 1.5 s with the GPU sharpen.
While the mode is on the panel is not the frame's — it still holds
the last-opened frame's edit — so nothing reads it: the save and
develop timers are stopped on the way in, undo, redo, the history's
rows, a snapshot and a preset act on the frame's sidecar alone and
then leave (the SNAPSHOTS and HISTORY sections are hidden in the
mode, and Ctrl+Z is the way those two are still reached), and a
snapshot taken is of the sidecar's current state. A control reached
for is the one exception, and §80's rule: the slider starts the
develop with the slider as set. The panel's edit before and after the
press are compared leaf by leaf through the edit's JSON, and the
leaves that moved are laid over the culled frame's own edit
(`overlay_changes`, tested); a section-level merge would have carried
the other frame's contrast across with the shadows. Export is
disabled in the mode, since the worker's open file is not the
selection's.

**Rejects out.** "Move N rejects..." on the CULLING section opens a
sheet that says how many frames, that their sidecars go too, and the
folder by its whole path (`<shoot>/rejects`), and that nothing is
deleted and the only way back is a move by hand. A file of the same
name already there, the raw's or the sidecar's, leaves the frame
whole where it is and is said so: never a raw moved and its sidecar
left, and never a file written over. The browser's
list is rebuilt, a thumbnail still owed is asked for again, and a
selection that went with them moves to the nearest frame left.

Left out: the previews are not cached on disk keyed by hash (§72's
cache is not there yet; a second pass through a shoot decodes again,
at 50 ms a frame); the compare view has no drag-to-pair, the set is
always a run; the previews are made at the view's size when the mode
is entered and a resize does not remake them; the two-thread duplicate
race for screen-size decodes is tolerated (one wasted decode at worst,
on a selection that moves in the microsecond between a thread's
finish and its delivery); a frame whose decode failed is tried again
only when it comes round as the selection; and rejecting a frame
under No rejects outside the mode develops the next, which is the
filter's meaning but two seconds of it. `--cull`, `--cull-compare`
and `--filter` are user-facing flags; the ones that press keys for a
capture (`--time-cull`, `--cull-develop`, `--ask-rejects`,
`--move-rejects`) are hidden.
