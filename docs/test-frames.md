# Test frames to shoot

Real frames the roadmap's work needs before any of it is called done:
the brackets for stacking (notes §71) and the set for the camera match
(`docs/camera-match.md`). Unit tests run on synthetic data made from
one existing raw; these are for what synthetic data cannot show.
`~/Pictures/Test` holds single frames and no brackets as of 2026-09-26.

Tick a set when it is in `~/Pictures/Test` under its own folder.

## Camera match

One body, one fixed picture style, every adaptive setting off, 20 to
40 frames. The fit learns the style from the maker's embedded JPEG, so
anything the camera varies by scene turns into noise in the fit.
Nothing in `~/Pictures/Test` qualifies today: the R6 II frames are all
Picture Style Auto, the R5 II has three Faithful frames with ALO off,
the DNGs carry Adobe's preview rather than the camera's, and the GFX
has three frames.

- [ ] Canon (the R5 II or R6 II): Picture Style Standard, Auto
      Lighting Optimizer off, Highlight Tone Priority off. Check with
      `exiv2 -g CanonPr.PictureStyle -g CanonLiOp -pt FILE`
- [ ] Across the light: daylight, open shade, tungsten, mixed. Skin in
      several. Group by white balance setting if the light is mixed
- [ ] A few frames deliberately a stop or two bright and dark, so the
      fit sees the whole tone range, and one with a clipped sky
- [ ] Saturated subjects: fruit, fabric, paint chips, a red car, a blue
      sign. Ordinary frames leave the LUT's corners empty
- [ ] A ColorChecker in three or four of them, if there is one; useful
      here, required for the chart profile that follows
- [ ] The same frames rendered to Faithful in Digital Photo
      Professional, no reshoot: a second style says whether the fit
      reads the style or only the camera
- [ ] The same shoot on the GFX in one film simulation at DR100, if
      there is time; the other simulations from in-camera conversion.
      The second body says whether the script holds across makers

Shows: the residual after matrix, curves and LUT; whether a held-out
frame fits; where the develop's clip before the look slot loses the
camera's shoulder.

## Stacking

Same body and lens throughout, raw only, one session covers the lot.

### HDR, tripod

- [ ] A high-contrast scene, five frames at 2 EV steps, shot with the
      camera's auto bracket
- [ ] The same scene, the same five frames, set by hand. Both EXIF
      paths get exercised
- [ ] Something moving in the frame: foliage in wind, or a person
      walking through. For the deghosting

Shows: how the bracket steps land in EXIF; real ghosts; whether the
longest frame's flare and sensor bloom pollute the merge.

### HDR, handheld

- [ ] The same scene, three frames at 2 EV, handheld

Shows: real shake, which has rotation and a little parallax; rolling
shutter skew if the electronic shutter was on.

### Focus stack, tripod

- [ ] A close subject with a deep near-to-far run, ten to fifteen
      frames stepped by the camera's focus bracketing
- [ ] The same, stepped by hand on the focus ring
- [ ] A hard near edge over a distant background in the frame. For the
      halo case
- [ ] A smooth surface with nothing sharp in it, such as a wall or sky.
      For the sharpness map's behavior where there is nothing to pick

Shows: the actual magnitude of focus breathing on the lens; halos at
real occluding edges.

### Panorama

- [ ] A handheld sweep of four to six frames with a third overlap
- [ ] The same sweep on a tripod, rotated with care about the lens, for
      a clean baseline

Shows: lens distortion across the overlap; parallax from not rotating
about the entrance pupil; exposure drift between frames.

## Order of need

The camera match set comes first: v0.6.0 is ahead of v0.8.0 on the
roadmap and the trial needs no engine code, only the frames. Of the
stacking sets the tripod HDR is the only one wanted before the first
piece of engine code, since tripod HDR ships first. The rest follow the
roadmap's order: focus stack, handheld HDR, panorama.
