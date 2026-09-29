# 52. Presets, and Lightroom's (2026-09-17)

Asked for on the roadmap: presets, and maybe an import of Lightroom's.
Both are in.

**What a preset is.** A named part of an edit: a list of the sections
it carries and a whole `Edit` with those sections set and the rest at
their defaults. Laid over a picture's edit, the carried sections
replace the picture's and everything else stays, the crop, the
retouch and the white balance included unless the preset carries
that last. Twelve sections can be carried, the panel's Develop
sections plus the adjustments; the geometry and the retouch cannot,
being one picture's own. The adjustments come across whole, masks and
looks, in place of the picture's, with fresh ids. The file is JSON,
`.gcp` beside the sidecar's `.gcd`, one a preset, under
`$XDG_CONFIG_HOME/greycard/presets`; the edit inside goes through the
same `migrate` as a sidecar's, so a preset outlives a schema change
(a version 1 preset's `noise.enabled` reads as the profiled pass, as
§48 says). The name inside is the name; the file's is a slug of it,
and a file with no name inside is called after its stem, so a preset
copied from elsewhere is listed as it came.

**The panel.** A PRESETS section at the top of Develop: the names, a
click laying one over the edit as one step in the history (undo takes
it back whole), a bin at each row's end. Save... opens a sheet with a
name and a switch a section; on to start are the sections a preset
carries by default (not the white balance, the lens, the demosaic or
the adjustments, which are more a picture's than a look's) and that
the edit has changed from the default, so what is saved is what was
done and applying it does not reset the rest. Import... asks the
desktop for a `.xmp` or a `.gcp` and puts it in the store. What the
sidecar holds, the panel is the truth for, so the apply reads the
panel first, records that, then the preset over it, and hands the
result to what undo and redo use: `take_current`, factored out of the
undo closure, shows the sidecar's current edit, writes the sidecar
and develops if the engine's part changed. `--preset NAME` on the
editor's command line lays one over the first file on opening, for a
screenshot or an export.

**The CLI.** `greycard presets` lists the store; `--import FILE...`
brings preset files in and says what each carries and what had no
place here; `--apply NAME FILE...` lays one over each file's sidecar
as a step in its history, which is the batch: a folder of raws given
a look in one line, then `greycard-ui --export` for each. A file that
has the preset already is left alone and said so.

**Lightroom's.** An `.xmp` preset is the `crs:` namespace's keys as
attributes or child elements of an `rdf:Description`, the tone curves
as `rdf:Seq`s of "x, y" on 0..255, the name in an `rdf:Alt`. Read
with roxmltree, now a workspace dependency shared with the lens
crate. Lightroom's sliders are mostly -100..100 over a tone pipeline
this engine does not have, so every mapping is a scale chosen to land
about where the slider lands there, and the file header and this
note say so: exposure in stops as it is; contrast ±100 to a slope of
0.5..1.5; highlights and shadows ±100 to ±1 stop of the tone curve's
shifts, whites to ±0.75, blacks to ±0.2 of mid grey; the point
curves divided by 255; the eight HSL bands onto the mixer's eight
(hue ±100 to ±30°, saturation and luminance to ±1), vibrance at half
weight and saturation at full added to every band, black and white
as every band to grey with the grey mixer as the bands' luminance;
split toning and the color grade's mid-tones onto the wheels, the
HSL hue turned into an Oklab hue by way of the saturated sRGB color
of that angle (a wheel at no saturation is the default wheel);
sharpening's amount to the sharpen's switch only, since an unsharp
mask's radius and detail are not a deconvolution's; luminance noise
reduction onto the profiled denoiser's strength; the post-crop
vignette's amount ±100 to ±2 stops, its midpoint, feather and
roundness as fractions; grain's amount as a fraction and its size
over 50, so Lightroom's 25 is the engine's default cell; the lens
profile's switch and the manual distortion's sign turned, since a
positive corrects a barrel there and a negative does here. White
balance by name (Daylight, Cloudy, Shade, Tungsten, Fluorescent,
Flash) at the usual kelvins, Custom as its kelvin and its tint at
-150..150 onto Duv with the sign turned; a JPEG preset's white is an
offset with no kelvin in it and is passed over. What has no place,
Texture, Clarity, Dehaze, the parametric curve, the color noise
reduction, the defringe, the calibration, a camera profile that is
not Adobe's, and the masks, is named to the user in the status line
and on the CLI, set to anything, so it can be done by hand. A preset
carries the sections whose keys the file had, whatever their values,
which is what Lightroom's own save sheet means by ticking a group.
Tested on a made-up preset with every group set, a black and white
one, one written as elements, and one with a JPEG's white; checked
in the editor with the made-up one over a church interior, which
came out warmer, half a stop brighter, and vignetted, as it asked.
