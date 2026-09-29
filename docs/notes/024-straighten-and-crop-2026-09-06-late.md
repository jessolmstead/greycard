# 24. Straighten and crop (2026-09-06, late)

The top of our roadmap. Asked, we wanted all three
together: a straighten angle, a level tool that takes a line dragged
along a horizon or a wall, and aspect presets with a custom ratio and
65:24 by name, plus a portrait toggle. `crates/greycard-edit/src/
geometry.rs` holds the edit and its maths; `crates/greycard-ui/src/
geometry.rs` is the CPU resample; the shader does the same; a GEOMETRY
section on the panel and an overlay on the viewport edit it.

**The model.** The picture turns about its center by an angle,
degrees counter-clockwise on the screen (a test draws a line and
checks which way it goes, since y runs down and every sign here has
two readings); the leveled plane is the source plane so turned; the
crop is a rectangle of that plane, in fractions of the source's width
and height so an edit fits any size of the same frame. The frame the
viewport shows and the export writes is the crop, or, with none set,
the largest rectangle of the source's shape that lies wholly on the
turned source, found by bisection on its scale about the center.
While cropping, the viewport shows the turned source's whole bounds
instead, with the crop drawn over it.

**Where it acts.** As a view transform, not a develop op: the shader
maps a frame pixel to the leveled plane to the source through the
turn and samples there, so the angle is live on the slider; the
export does the same on the CPU on the developed image before the
finish, and the worker's cached develop is untouched by it. When
turned, both sample with Catmull-Rom (sixteen taps; bilinear would
soften a picture that was just sharpened, §21); unturned, a crop is
whole pixels. The histogram's pass renders the frame, so the bins
are the crop's. The viewport at 1:1 and the export's crop with a 4.5
degree turn and a crop from a sidecar differ by 0.06 percent RMSE.

**The panel and the overlay.** Crop enters crop mode: shade outside
the crop, a rule of thirds inside, eight handles and the whole to
move. A handle reports the pointer's travel since the press in window
coordinates, and the window applies it to the crop as it was when
pressed, so a handle moving under the pointer does not count its own
motion twice, the mistake every first crop tool makes. A drag is
rejected rather than clamped when the crop would leave the turned
source: it stops at the edge. Corner drags with an aspect held let
the larger of the two motions win and anchor the opposite corner;
edge drags keep the crop centered on the other axis. Release records
one history entry. The angle slider, the aspect (Free, Original,
1:1, 3:2, 4:3, 5:4, 16:9, 65:24, or Custom typed as `W:H`) and the
portrait toggle re-fit the crop: it keeps its place if it still fits
and has the shape asked, else the largest of that shape about its
center, no bigger than it was, so turning back and forth does not
grow a crop the user shrank. Level: a drag along a line that should
be level or plumb; the nearer of horizontal and vertical is taken and
the correction added to the angle in force.

**Not yet.** The overlay cannot be driven from the command line, so
its feel is our call. Lens corrections and perspective are
separate ops. Flip and 90 degree turns belong here and are a few
lines when wanted.
