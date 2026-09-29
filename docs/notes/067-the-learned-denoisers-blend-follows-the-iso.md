# 67. The learned denoiser's blend follows the ISO (2026-09-18)

`Noise::learned_strength` had one constant default, 1.0: full blend
whatever the tier. At base ISO that meant "best" replaced the
demosaic's answer wholesale though a clean frame barely needed it,
and §61 found fabric weave and surface texture gone from an ISO 100
frame for it.

`Noise::blend_for_iso(iso: Option<u32>)` in `greycard-edit`, a pure
function of the number (the crate still knows nothing of pictures or
files): a ramp in log2 of the ISO, stops rather than ISO itself
since noise is a stops quantity and a linear ramp would spend nearly
its whole range under ISO 800; 0.35 at ISO 200 and below, 1.0 at
3200 and above, straight between; the old constant when the ISO is
unknown, so a file with no EXIF behaves as before.

Wired in where a fresh edit is made for a file with no sidecar. The
CLI's `presets --apply` reads the ISO through `decode::probe_path`,
a metadata-only read built for the trainer's survey (§37). The
editor first did the same in its folder scan, and review measured
the probe at 3 to 10 ms a file warm and 39 ms cold, which on a
folder of a few hundred sidecar-less raws is seconds on the main
thread before the window shows. So the scan only marks which files
are raws starting fresh, and the worker seeds the blend on the
file's first open, where the frame is decoded anyway and carries
its ISO: into the edit it develops, and back to the editor, which
puts it in the sidecar and on the panel's slider. A sidecar's own
blend is never touched, however it got there; the history's
"Original" label ignores this one field when deciding a state is
the fresh one.

Presets needed a fix, not a check. `Section::Noise` carried the whole
`Noise` struct, and a Lightroom import, built from `Edit::default()`
with no notion of the learned denoiser, would have stamped the
constant back over a file's ISO blend on apply; and every fresh raw
under ISO 3200 would have read as "noise changed" in the save
sheet. The section now carries only the profiled pass (`enabled`,
`profiled`, `strength`); the learned tier and its blend are the
file's own, like the white balance and the lens, never a preset's to
move between pictures.

Checked with `--snapshot` on an ISO 125 frame (Blend 35%) and an ISO
1000 frame (73%, the ramp's 0.727), both labeled "Original" in the
history; the CLI's `--ai-denoise` help says the CLI runs at full
blend while the editor's default follows the ISO.
