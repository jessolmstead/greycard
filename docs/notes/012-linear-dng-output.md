# 12. Linear DNG output (2026-09-05)

The pre-processor's product exists: `greycard develop FILE --dng OUT.dng`.

- **What is in the file.** Demosaiced camera-space RGB, levels normalized,
  16-bit, black 0, white 65535, `PhotometricInterpretation = LinearRaw`,
  lossless JPEG tiles (`--uncompressed` to skip). Nothing else is developed:
  no white balance, no matrix, no tone. `AsShotNeutral` (reciprocal gains),
  `ColorMatrix1/2` and `CalibrationIlluminant1/2` (the warmest and coolest
  calibration the file carries, the same pair the engine's own profile uses),
  `Orientation`, EXIF and lens tags from the source, an sRGB JPEG preview
  and a thumbnail. The consumer white balances and color-transforms it
  exactly as it would the original file.
- **Why camera-native rather than balanced.** Gains are applied before the
  demosaic (algorithms assume balanced channels) and divided out after. The
  DNG path does *not* clip at 1.0 in balanced space, unlike `develop`: a
  saturated sensor channel must arrive at white level in its own channel so
  the consumer's highlight handling sees the fact. Consequence: developing
  the linear DNG and developing the original differ only at blown edges, and
  only because of where the clip happens. Checked on the R6 II orchids and
  the R5 II moon (Rotate270 survives): mean difference 0.01–0.03 of 255,
  fewer than 0.02% of samples differ by more than 1.
- **Engine shape.** `develop::demosaic` is the new stage boundary: it
  returns a `CameraImage` (camera space, 0..1) plus the white balance it was
  demosaiced under. `develop` is the same stages with the clip and the
  matrix. `dng::write_linear_dng` takes the frame, the camera image and the
  gains; rawler's `DngWriter` does the container and the LJPEG, we set every
  level and color tag ourselves and leave its matrix selection unused.
  `RawlerDecoder::decode_path_with_metadata` carries rawler's `RawMetadata`
  through untouched for the EXIF; it is rawler's type, not interpreted.
- **Cost.** Bilinear twice when `--dng` is combined with `--output` or
  `--preview` (the DNG path and the develop path each demosaic). Fine for
  now; revisit when the real demosaic lands and is not free.
- **Not yet.** `ForwardMatrix` (rawler does not surface it), `BaselineExposure`,
  embedding the original RAW, and any check in a third-party editor. The
  round-trip test decodes the DNG with rawler; the next verification is
  opening one in darktable or RawTherapee and comparing against the source.

---
