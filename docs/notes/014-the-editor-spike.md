# 14. The editor spike (2026-09-06)

The engine's quality list being empty (§13v), the toolkit spike from §5
started, Slint first as planned. `crates/greycard-ui`, binary
`greycard-ui FILE|DIR`: a Slint window with the three panels, a
thumbnail strip on the left, the viewport in the middle, the controls
on the right; the engine on a worker thread, the newest develop
winning; the developed image on the GPU as an `Rgba16Float` texture
in the working space, drawn by a twenty-line shader (zoom and center
as a pixel mapping, exposure as a gain, clip in the working space, the
matrix to linear sRGB, the target's sRGB format encoding) into a
texture the size of the viewport that Slint shows as an `Image`.
Scroll zooms about the cursor, drag pans, double-click fits.
`--screenshot FILE` writes the viewport after the first develop and
quits, which is how the results below were checked without a
screenshot tool on GNOME Wayland.

**The texture import works on the NVIDIA Wayland machine.** Slint
1.17.1's `unstable-wgpu-29` feature with the femtovg-wgpu renderer,
`BackendSelector::require_wgpu_29` with the texture limit raised to
16384, the engine's texture created on Slint's device inside the
rendering notifier, `slint::Image::try_from(texture)` each frame. The
24 MP R6 Mark II frame and the 45 MP R5 Mark II DNG (8192 wide, above
the default limit) both render, colors matching the CLI's preview.
Per frame the viewport costs a tenth of a millisecond of encoding; the
upload after a develop is 48 ms for 24 MP (190 MB of half floats)
once the float-to-half conversion moved to the worker (it was 340 ms
on the UI thread). A 24 MP develop is 1 s without denoising and 5.4
with, on the worker, the window live throughout. That answers the
first of the three questions in §5 with a yes, and the wgpu major is
the constraint it leaves: the engine's GPU work, when it comes, must
use the wgpu Slint pins (29 today), which moves with Slint's minor
releases.

**The canvas interaction was not painful.** Zoom about the cursor,
pan and fit are a `TouchArea` with three callbacks and forty lines of
Rust; the state (zoom, center, the pending image) is a struct on the
UI thread the callbacks and the rendering notifier share through a
`RefCell`. The one thing to know: Slint's `Image` is not `Send`, so a
thumbnail crosses from the worker as bytes and becomes an image in
the event loop.

**The slider panel is not judged yet.** It is the standard widgets on a
dark background with no design pass, and the plan's design-tokens week
(§5) was skipped to get here. Whether it looks finished, and how long
it takes to, is the question left open, and it needs eyes on the
window rather than a screenshot of the viewport. The second spike (egui
with Rerun's design layer) stays in reserve for if Slint fails that
judgment.

**What the spike leaves out**, all deliberately: a tone curve (the
viewport is linear with a clip), color management of the output (the
shader assumes an sRGB monitor), a histogram, any edit beyond exposure,
white balance, denoise strength and demosaic choice, and an edit
schema (the edit struct is five fields in the UI crate; the engine
still takes `DevelopSettings`).
