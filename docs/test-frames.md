# Test frames to shoot

Real brackets the stacking work (notes §71) needs before any of it is
called done. Unit tests run on synthetic brackets made from one existing
raw; these are for what synthetic data cannot show. `~/Pictures/Test`
holds seven single frames and no brackets as of 2026-09-18.

Same body and lens throughout, raw only, one session covers the lot.
Tick a set when it is in `~/Pictures/Test` under its own folder.

## HDR, tripod

- [ ] A high-contrast scene, five frames at 2 EV steps, shot with the
      camera's auto bracket
- [ ] The same scene, the same five frames, set by hand. Both EXIF
      paths get exercised
- [ ] Something moving in the frame: foliage in wind, or a person
      walking through. For the deghosting

Shows: how the bracket steps land in EXIF; real ghosts; whether the
longest frame's flare and sensor bloom pollute the merge.

## HDR, handheld

- [ ] The same scene, three frames at 2 EV, handheld

Shows: real shake, which has rotation and a little parallax; rolling
shutter skew if the electronic shutter was on.

## Focus stack, tripod

- [ ] A close subject with a deep near-to-far run, ten to fifteen
      frames stepped by the camera's focus bracketing
- [ ] The same, stepped by hand on the focus ring
- [ ] A hard near edge over a distant background in the frame. For the
      halo case
- [ ] A smooth surface with nothing sharp in it, such as a wall or sky.
      For the sharpness map's behavior where there is nothing to pick

Shows: the actual magnitude of focus breathing on the lens; halos at
real occluding edges.

## Panorama

- [ ] A handheld sweep of four to six frames with a third overlap
- [ ] The same sweep on a tripod, rotated with care about the lens, for
      a clean baseline

Shows: lens distortion across the overlap; parallax from not rotating
about the entrance pupil; exposure drift between frames.

## Order of need

The tripod HDR set is the only one wanted before the first piece of
engine code, since tripod HDR ships first. The rest follow the roadmap's
order: focus stack, handheld HDR, panorama.
