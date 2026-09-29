# 10. Kelvin slider: display Kelvin, store the offset

- **Display Kelvin, store the offset.** The stored value stays an offset, so
  sidecars, presets, a Lightroom importer, copy/paste and the mask variant of
  the panel are all untouched by the readout.
- The absolute readout needs as-shot CCT and Duv delivered with the loaded
  image, and only where a profile actually resolved; masks and non-managed
  files keep the plain readout. A slider that shows a transformed value needs
  the transform both ways, since a user types into the box — and typed Kelvin
  must apply on Enter or blur, never per keystroke, because "32" on the way to
  "3200" is a real temperature. Travel stays ±150 mired (from 2986 K:
  2062–5408 K; from 5500 K: 3014–25000 K, clamped at the model's limit).
- Wherever the mapping is duplicated between a front end and the engine, the
  two have to be kept in step, which is an argument for not duplicating it.
- Tint stays unitless for now (Duv in thousandths is an option).
- **Follow-up:** rework the eyedropper into a command that samples the
  camera-native buffer at the clicked point and solves for the CCT/Duv that
  make it neutral; `rawcolor` has the chromaticity → temp/tint half. This is
  what makes the absolute readout pay off.

---
