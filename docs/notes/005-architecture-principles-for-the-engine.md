# 5. Architecture principles for the engine

In order of how much they matter.

1. **Engine as a library, UI as a separate crate.** No Tauri/React/egui in the
   engine. CLI is the first front end.
2. **One render graph for everything.** Preview, export, thumbnail, swatch are
   the same graph at different resolutions.
3. **Scene-referred linear working space, wide gamut, from day one.**
   Linear Rec.2020, D65 (see §6). Color-managed at every boundary, display
   transform last, no stage may assume sRGB.
4. **CPU reference for every operation, GPU as accelerator**, held to it by
   tests. Golden-image tests.
5. **Typed, versioned edit schema in physical units:** Kelvin, Duv, EV,
   degrees. Migrations on schema change. An edit made today renders
   identically in five years.
6. **Camera profiles as first-class data.** DNG dual-illuminant by default,
   DCP and ICC loading, a path for user-measured profiles.
7. **Native GPU surface for the preview.** No readback, no encode (see §7).
8. **Display color management.** Read the monitor ICC (colord / Wayland
   color-management protocol), fall back to sRGB. Build the sampling shader to
   accept a 3D LUT from day one so ICC is a data change. HDR output later is a
   surface-format change, not a pipeline change.
9. **Edit history, not sidecar overwrite.** Snapshots and versions per image.
10. **Fewer features, all correct.** No AI masking, inpainting or generative
    anything at first. Decode, develop, tone, color, local masks, crop,
    export, library.
11. **Linux first, Wayland first.**
12. **Tests from the first commit.**
13. **Every op must tolerate negative channel values** (wide gamut in float)
    or be preceded by gamut mapping. Guard logs, powers and divisions.
    Gamut-map to the display at the end.

The moat is unchanged: color pipeline, decode breadth, catalog at scale,
tethering. Tethering remains the clearest differentiator on Linux.

### Decoding: rawler as decoder, behind a trait, never as developer

- **Use it.** Raw decoding is a treadmill (every new body is a format quirk)
  and rawler is the best Rust option: actively maintained as the core of
  DNGLab, no FFI, and it hands over exactly what the engine needs: sensor data
  with the CFA pattern, black/white levels, as-shot coefficients, the full set
  of illuminant-keyed calibration matrices, crop, orientation, lens data. It
  also writes DNG, so the pre-processor's linear DNG output comes from the same
  dependency.
- **Decoder only.** rawler's develop path is the thing to work around, not
  build on: daylight matrix regardless of illuminant, WB and matrix applied
  in one go, basic demosaic. Everything after "sensor values +
  metadata" is an engine op with a CPU reference and a GPU implementation:
  levels, WB in camera space, demosaic (RCD/AMaZE quality is the target; it's
  a visible differentiator), camera matrix, working space.
- **Behind a trait**, bytes in, `RawFrame` out (sensor buffer, CFA layout,
  levels, as-shot coefficients, calibrations by illuminant, optional forward
  matrices, crop, orientation, metadata map). Roughly rawler's own struct with
  the develop functions removed, which says the boundary is in the right place.
  Reasons: coverage (rawler's camera list is narrower than LibRaw's and lags on
  new bodies; a LibRaw FFI backend can fill gaps per format), license (LGPL-2.1
  static linking; behind a trait it can become a dynamic library or a separate
  process if a permissive-only stack is ever needed), replacement (a native
  decoder, or libopenraw's Rust rewrite, slots in later).
- RapidRAW doesn't use upstream rawler; it pins its own fork with patches
  (fast demosaic scaling, multi-exposure). That is the normal fate of a
  decoder dependency in an editor and another argument for arm's length.
  Contribute camera-support fixes upstream to rawler, not to a fork: they
  benefit the pre-processor, the engine and RapidRAW alike.

---
