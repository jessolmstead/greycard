# 84. ProPhoto as a working space: no (2026-09-19)

The roadmap carried the question as an aside since the export's
color space choice. §6 chose linear Rec.2020 and nothing since has
weakened the case; what has changed is that each place ProPhoto
might seem needed now has a concrete answer.

A switch would cost more than it gives. The working space is no
longer one constant in practice: it is baked into every shader and
CPU reference, every checked RMSE in these notes, the Oklab
conversion, the mixer's band centers, the vibrance's skin protection
at 55 degrees, the grain, and so the meaning of every sidecar. An
edit does not render the same under different primaries, because
per-channel operations (curves, contrast, saturation) twist hue
differently, and ProPhoto's imaginary primaries make the twist
larger (§6). A choice of space means every edit and every preset
carries which one it was made in, and the Lightroom import lands
differently by setting. Two spaces is two of everything to keep
correct, for a project whose claim is that the pipeline is tested to
the pixel.

What is asked for when ProPhoto is asked for, and the answer to
each: "does it hold every color?" In float, yes; Rec.2020 values go
negative and above one and every op tolerates that (§5 rule 13), so
the working space clips nothing, and ProPhoto's extra volume is
mostly colors no camera records and no display shows. "Will my
Lightroom edits match?" Not in either space, since the tone
pipelines differ and §52 maps the sliders by chosen scales; ProPhoto
would not make Lightroom's curve appear. "DCPs are defined in
ProPhoto." They are, and §78 applies the hue map in ProPhoto HSV as
a stage inside the profile and comes back to Rec.2020; the same for
any LUT made for a ProPhoto input. One matrix each way.

Where ProPhoto belongs, and already is: among the colorants an
export profile is built for (§50), for files handed to a workflow that expects it. So: supported
wherever an operation or an output is defined in it, never as the
space the edit lives in. The aside is struck.
