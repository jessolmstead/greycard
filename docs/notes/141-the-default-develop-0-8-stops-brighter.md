# 141. The default develop, 0.8 stops brighter (2026-09-22)

The first two reference frames (§85) settled one number before any
slider. Developed with nothing touched and read back as linear
luminance, a Canon R6 II frame of a white watch dial came out with a
median of 0.222 where the camera's embedded JPEG has 0.386 and
Lightroom 0.402: 0.8 stops under both, top to bottom, the dial a
middling grey. The raw's brightest sample sits 1.8 stops under the
clip, so this is not §85's open question about the shoulder's white;
it is where the picture is placed. Matching Lightroom's +0.8 took
+1.15 here and still left the mid-tones 0.44 stops short, and the
slider itself responds the same in both editors, so the gap is the
starting point and not the scale.

**A baseline, added where the exposure becomes a gain.**
`finish::BASELINE_EXPOSURE` is 0.8 stops, added to the exposure in
`finish_pixel_with`, in `pick` and in the viewport's uniform, and
nowhere else. The guide plane is the scene's before any exposure and
the exposure is added to it at the pixel, so the baseline moves the
tone equalizer's regions exactly as the slider does; the picture at
zero is the picture the slider at +0.8 gave before, locals and all.
It lives in the finish, not in core or the edit, because it is a
choice about the display rendering, which is the consumer's.

**Stored edits come up brighter, on purpose.** The number in a sidecar
still means stops from the default develop; the default is what moved.
No schema version for it: a migration that took 0.8 off every stored
exposure would keep the old, dark default on every frame ever opened,
which is the thing being fixed, and nothing has shipped.

**Measured after.** The watch frame's mean encoded value is 142.1,
Lightroom's 142.1, its median 0.398 against 0.402; its top is still a
little short (0.555 against 0.597 at the 99.5th percentile) and its
darkest tenth a little lifted (0.028 against 0.021), which is a flatter
curve than Adobe Color and a question for the reference set. The GFX
ferry frame, which was only 0.3 stops under its camera's JPEG, is now
half a stop over Lightroom at the median (0.056 against 0.038) and
still short of white at the top. One number for every camera is where
the set of eight starts, not where it ends: if the makers' JPEGs keep
disagreeing by camera, the baseline is a per-camera value like DNG's
`BaselineExposure`, and the frames will say so.

**The sliders at full, measured on the same two frames**, by
percentile band of each editor's own picture so the lens geometry
does not matter. Shadows +2 against Lightroom's +100 agrees within
about 0.2 stops everywhere but the darkest tenth, where Lightroom
lifts half a stop more. Highlights -2 against -100 is a different
shape: Lightroom's reaches down to about the 25th percentile and is
strongest from the 40th to the 95th, -0.7 stops on the watch, while
greycard's does nothing under the median and is strongest at the very
top, where it pulls harder than Lightroom's. That is why it reads as
weak; the ramp is for after the baseline, since both ramps hang off
mid grey and this change moves what sits there.
