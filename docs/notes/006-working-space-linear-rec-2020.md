# 6. Working space: linear Rec.2020

- **ProPhoto (ROMM):** D50, blue and green primaries imaginary, ~1/8 of the
  encoding volume non-physical. Chosen historically for integer pipelines
  where out-of-gamut clips. In float, "contains every color" stops mattering;
  what matters is how per-channel ops (curves, saturation, contrast) behave,
  and imaginary primaries make their hue twist larger and less predictable
  (why Adobe added hue-preserving tone mapping). RawTherapee and Lightroom use
  it.
- **Rec.2020:** real (monochromatic) primaries, D65 matching sRGB, displays and
  the DNG daylight calibration; covers nearly all of Pointer's gamut; it is the
  output ecosystem (HDR, Rec.2100); AgX and other tone mappers were designed
  around it; float16 precision is spent on real colors. darktable's default.
- **ACEScg (AP1):** slightly wider, white near D60, built for VFX interchange.
  Non-standard white is a cost with no matching benefit for a photo editor.

`rawcolor` supports all three; in the engine the working space is one
constant. Choosing Rec.2020 doesn't lock the door.

---
