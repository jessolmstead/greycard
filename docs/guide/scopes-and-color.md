# Scopes and seeing color right

This chapter covers the tools that tell you what is really in the
picture: the histogram and scopes at the top of the panel, the
clipping warnings, soft proofing, and your monitor's profile. Read it
when you need to judge exposure or color by more than eye, or when
greycard's colors don't match another app.

## The histogram and the scopes

The scopes sit at the top of the right panel, under the file's name,
and stay there while the sections scroll. They are hidden while you
cull. Pick one from the row under it:

| Scope | Shows |
|---|---|
| **RGB** | the histogram: how many pixels at each level, dark at the left, bright at the right |
| **Wave** | a waveform: brightness up the scope against the picture's columns across it, so you can see where in the frame the bright and dark parts are |
| **Parade** | the same as Wave, with red, green and blue side by side |
| **Vector** | the colors on a wheel: grey in the middle, stronger color further out, with a line where skin tones fall |

The scope you pick is remembered. The histogram also shows behind the
curve in **CURVES**.

The scopes read the picture as it will be written, in the color space
set in the export sheet, not the camera's raw data.

### Reading one mask: Selection

**Selection**, beside the scope choices, makes every scope count only
the pixels in the chosen mask. On a skin mask, **Vector** shows the
skin alone against the skin line; on a face mask, **Wave** shows the
face's level without the background over it.

It works on the **Masks** tab with a mask chosen that has a shape, and
is greyed otherwise. It is off each time greycard starts. While it is
on, the clipping lamps on the histogram follow the mask too. See
[Masks](masks.md).

## Clipping

On the **RGB** histogram, two small squares at the top corners light
when part of the picture is pure black (left) or pure white (right).
Their color says which channels clip: red, green or blue for one,
yellow, magenta or cyan for two, white for all three.

Click a square to paint that end's clipping over the picture: blue
where it is black, red where it is white. Press **J** to turn both on,
or both off. The choice is remembered.

The lamps say what the exported file will clip after the tone curve,
not what the camera's sensor clipped. The painted warning always
covers the whole frame, even with **Selection** on.

## Soft proofing

A soft proof shows how the picture will look in a narrower color
space, or on a printer, before you export. Press **S**, or use the
switch on **SOFT PROOF** at the end of the **Develop** tab.

- **sRGB**, **Display P3**, **Rec.2020** or **File**: the space to
  proof against. **File** takes an ICC profile, such as a printer's,
  through **Choose...**.
- **Perceptual** or **Relative**: how colors outside the profile are
  brought in. For sRGB, Display P3 and Rec.2020 the two look the same;
  the choice matters for a printer profile.
- **Gamut warning** paints grey where a color falls outside the
  profile.

The profile and the intent are remembered; whether the proof is on is
not.

## Your monitor's profile

The **MONITOR** section, after SOFT PROOF, sets the profile greycard
uses to show colors on your screen. A line under it says what the
profile changes.

| Choice | Use it when |
|---|---|
| **System** | on Linux, to use the profile your desktop has for the monitor; with several monitors, pick yours below |
| **sRGB** | your monitor is an ordinary sRGB screen |
| **Adobe RGB**, **Display P3** | your monitor is set to emulate that space in its own menu |
| **File** | you have an ICC profile for the monitor, from a calibrator or its maker; pick it with **Choose...** |

On Windows and macOS, greycard cannot read the system's monitor
profile yet, so **System** shows colors as sRGB. On a wide-gamut
screen colors then look too saturated. Pick the monitor's ICC profile
with **File** (Windows keeps them in
`C:\Windows\System32\spool\drivers\color`, macOS in
`/Library/ColorSync/Profiles`), or one of the named spaces if the
monitor is set to it.

**Canvas** sets the color around the picture: black or one of three
greys.

The monitor choice is remembered. It changes only what you see; an
export is never affected.

## How greycard handles light

You don't need this to edit, but it explains why the sliders behave
as they do.

greycard works on the light the camera recorded, not on screen
brightness. Every slider acts on that light, and only at the very end
does a display curve fit it to the screen. So:

- **Exposure** is a true stop: +1 doubles the light, in the shadows
  and the highlights alike.
- Bright parts keep their detail through the edit; they are rolled
  off gently by the display curve, not cut off at white early.
- Colors stay wide until the end, so a strong color edit doesn't hit
  a wall partway through.

The display curve is either **Per channel** (the default) or **AgX**;
the switch is at the foot of **LIGHT**. See [Develop](develop.md).

For the technically minded: the working space is linear Rec.2020.
