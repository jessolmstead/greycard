# 94. The filter's strength, and the mixer under a mono conversion (2026-09-19)

Two of §87's four, or rather one of them and a question §87 did not
ask. The strength is the one that was plainly just a number and it is
built as that section decided it. The other is what the color mixer
is for while the picture is black and white, which turns out to have
a cleaner answer than the one the roadmap's wording suggested.

**The strength is a field, not a wider range.** `BlackWhite` carries
a `strength: f32`, `serde(default)` of one, and `BlackWhite::stops`
multiplies the summed gain by it: `confidence * w * RANGE *
strength`. One is the tables exactly as §77 wrote them, which is what
every sidecar and preset written before the field meant, so nothing
migrates and `VERSION` stays at 3. Three is the top, and it is where
the Red filter's deepest cut, one stop at Blue as the table has it,
becomes the three a Wratten 25 costs a blue sky against a
panchromatic rendering; the Orange lands near a 15's two and the
Yellow near an 8's one on the way past. The alternative §87 weighed,
raising `RANGE` and dividing the tables, reaches the same depth and
changes what a stored weight means, which is a version bump and a
`migrate` case bought for nothing.

`BlackWhite::filter` still matches on the weights alone, so the row
of filters says Red at any strength and a hand-moved band still reads
as custom. That is the division the control is for: the row is which
conversion, the slider is how much of it, and the two do not argue.
The history says "Black and white strength 2.00x" when only it moved,
between the switch's line and a band's.

The one thing the strength does have to reach is the Weight slider's
reading. That row printed the stored weight with " EV" after it,
which was true while the strength was the one it could not be told
about: at 3.00x a weight of -1.00 applies three stops and a reading
of "-1.00 EV" is a lie about the picture. So it prints the product,
weight times strength — "+2.40 EV" for Red's own band at 3.00x —
and the handle no longer sits where its number says, which is the
correct way round: the handle is the setting, the number is the
effect, and the row under it says by how much they differ. Dropping
the unit and printing a bare coefficient was the other way; it keeps
the handle and the number together and makes the panel answer "how
many stops" with a multiplication, which is the question a
photographer is actually asking. Two doc comments went with it:
`RANGE` is stops per unit weight *at a strength of one*, which is
the strength at which it and the mixer's luminance slider still read
alike, and the module header's "a gain on that band's grey in stops"
is now in stops once the strength has scaled it.

The shader takes it as a second component of the `bw` uniform and
multiplies it into `look.bw_w[b]` where the bands are read, not where
the gain is summed. That is exact, since the strength is one scalar on
a sum that is linear in the weights, and it is safe because the black
and white is global: nothing but the picture's own section ever
writes those eight, the way a local adjustment writes into the
mixer's. The panel gets a Strength slider under Weight, zero to
three, a twentieth to a step, one by default and one after Reset,
read out to two decimals ("1.00x", "2.35x") rather than the panel's
usual trimmed number, because a multiplier that shuttled between "1x"
and "1.05x" would move the column it sits in.

**What the mixer was doing under the conversion.** Asked for as
"enabling the B&W tool should disable the color mixer", and worth
looking at what there was to disable. In `mix_with` the conversion
takes the last word on the Oklab pass: it sets a and b to zero and
the lightness to `lab[0] * light_scale * bw.light(...)`. So the
mixer's hue shift and its saturation could not reach the picture
already — §77's line that nothing downstream that *scales* chroma can
bring the color back covers them. But `light_scale` is the mixer's
own luminance slider, and it survives. Eight more band gains, on the
same eight bands, read from the same mean hue with the same
confidence, in the panel section directly above the one whose whole
job is eight band gains. Not a no-op, then: the same control twice,
in two places, with nothing in the picture to say which of them did
it. A sky pulled down by the mixer's blue luminance and a sky pulled
down by the conversion's blue weight are the same pixels, and only
the panel knows the difference.

So it is not that the mixer is harmless under a conversion and may as
well be switched off for tidiness. It is that it is a duplicate, and
the conversion is the one of the two that is named for the job.
Lightroom reaches the same place by another road, swapping its Color
Mixer panel for a B&W Mixer when the conversion goes on.

**The rule is in the edit, not in the panel.** `Edit::acting_mixer`
returns the zeroed, switched-off mixer while `bw.enabled`, and the
three places that build a look from an edit go through it:
`Baked::global` for the CPU finish and the export, the `View` the
viewport shader is handed, and `pick_stages`, so the droppers read
the mixer the render applied rather than the one the panel shows.
The panel's `mixer.enabled` is never written,
which is the whole argument for putting it here. The roadmap asked
for the switch to go off and for its old state to come back "if that
is recoverable"; nothing needs recovering if nothing was spent. The
setting is untouched, so it comes back exactly as it was when the
conversion goes off, whether that is a second later or a year later
through a sidecar. A preset or a Lightroom import that carries both
sections lands the same way the editor does. There is one history
entry for the one thing the user did, "Black and white on", because
that is the one thing that changed.

It is the picture's mixer only. A local adjustment's is its mask's
business: its luminance under a mono conversion is dodging by the
hue the pixel had, which is a thing a photographer means to do and
cannot say any other way, and its blend into the look is by the
mask's weight, not a section switch. So `acting_mixer` is about
`edit.mixer` and `finish_pixel_with` still adds each local's.

**The panel says so.** `Section` gained `superseded` and `Switch`
gained `force-off`: the switch draws off and takes no clicks, the
body dims to the same 0.45 a switched-off section dims to, and `on`
keeps what it was underneath. COLOR MIXER passes `bw-enabled &&
target == 0`, the target test because on the Masks tab that section
is a mask's and stays live. Reset is left alive, as it is on a
switched-off section, so the sliders can still be cleared. The
section reads as taken over rather than as broken, which is what it
is. Pick and Reset go dead with the sliders — Reset especially,
since on an ordinary switched-off section it is the one control that
should still work, and here it is the one that must not: a section
whose whole promise is that its settings are being held for later
cannot offer a button that throws them away, and writes a history
entry for a section drawn as doing nothing.

**The shader, checked twice.** §5's fourth rule says a GPU
implementation is held to a CPU reference, and §18 built that
reference and the 1:1 comparison that keeps it honest. But nothing
in `cargo test` had ever *read* `viewport.wgsl`: the pipelines are
built against a real device, so a typo in the shader was found by a
black viewport on somebody's machine rather than by the suite.
`render.rs` now parses and validates both shaders with naga, which
is already in the tree at 30.0.1 under wgpu 30 and needed only to be
named in dev-dependencies — one line in `Cargo.lock`, no new crate.
It catches an undefined identifier with the source line and a caret,
which is what it was worth adding for.

Then §18's own check, on 4Z4A1919 under Red at a strength of three,
which is the deepest this control goes and so the hardest place for
the two paths to disagree: the viewport at `--zoom 1` (940x802)
against the matching crop of the 8192x5464 export, display profile
off on both, export sharpening off, PNG so nothing is lost to a
quantizer. RMSE 0.0788 percent, 0.201 of 255; mean absolute
difference 0.040 of 255; and the maximum difference anywhere is
exactly 1 of 255, on 5.3 percent of the channel samples and no
sample worse. That is the two paths agreeing to the last code value
under the largest gains the section can apply — tighter than §18's
own 0.12 of 255 and 0.2 percent, because this frame needed no
half-pixel fudge: 8192-940 and 5464-802 are both even, so the 1:1
crop lands on whole pixels. Worth saying how one knows: a sweep of
the crop offset by a pixel each way puts RMSE at 51.7 in the middle
against 668 one pixel across and 1530 one pixel down, so the center
is not a coincidence of a soft picture.

**What it costs.** An edit that had both the mixer and the conversion
on renders differently than it did: the mixer's luminance no longer
reaches it. That is the change, it is deliberate, and it is the only
way the duplicate goes away. The sliders are still there and still
say what they said, so nobody loses work, and the picture they were
moving is the one the conversion's own weights move.

**And still not fixed.** §87's first, third and fourth stand: None
starts from perceptual lightness rather than a luma mix, the response
is flat above a chroma of 0.03 so a filtered sky darkens without its
gradient, and a band reaches its neighbors and stops. A Red at three
is now as deep as the glass and still flat across the sky, which
makes the third of those easier to see, not harder. Both of the ones
worth arguing about wait on §85's reference frames.
