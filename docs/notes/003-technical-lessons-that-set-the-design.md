# 3. Technical lessons that set the design

Things that cost real time to establish. Each of these is a test or a design
rule in the engine.

1. **Luminance normalization of the camera matrix.** Balanced camera white
   `(1,1,1)` must map to `Y = 1`. A raw ColorMatrix carries arbitrary magnitude;
   inverting it without rescaling is an exposure shift (+10%
   brightness, predicted `Y = 1.115` for the R6 II at 3553 K). rawler and the
   DNG SDK both normalize; `rawcolor` does too. A neutrality test that
   normalizes by max component lets this through, which is the lesson:
   **test absolute luminance, not just ratios.**
2. **DNG ForwardMatrix contract.** Defined on already white-balanced data, rows
   sum to D50 white. Do not apply the neutral again.
3. **Pipeline order:** raw → black/white level → WB gains in camera space →
   camera→XYZ with the illuminant-interpolated matrix → working space → tone.
   Anything that applies WB in the working space, or picks the matrix without
   reference to the scene illuminant, bakes in an error the sliders can't
   remove.
4. **One render graph.** An editor with ~14 render entry points rebuilds its
   adjustments at each one. Where the loaded buffer is camera-native and a
   shader's default uniform is pass-through, export of the open image takes
   that buffer straight through the stock pipeline: **no white balance at all
   in the output**, while batch export decodes through the daylight path and
   matches neither. The answer is to carry the profile with the pixels into
   every consumer, with a CPU port of the shader step for thumbnails and
   auto-adjust (they never touch the GPU, and they
   feed a cache shared between in-memory and from-disk sources, so flipping a
   GPU flag there risked poisoning the cache). Rule: any consumer of the base
   image must apply the transform. Better rule: there is only one consumer.
5. **CPU reference for every GPU op.** The CPU port is the only way the
   shader's arithmetic gets unit-tested. Test: camera white → unit white, grey
   stays grey, a blown pixel collapses to neutral at the brightness of the
   hottest channel.
6. **The pipeline assumes sRGB everywhere.** AgX takes the pipe as sRGB and
   converts to Rec.2020 internally; HSL, grading and LUTs assume sRGB too.
   Feeding it Rec.2020 reads as desaturation. Widening the working space in
   RapidRAW is a pipeline-wide rewrite.
7. **Unitless sliders and untyped JSON.** Temperature is −100..100 mapped to
   ±150 mired around the as-shot value (a constant shared with the Lightroom
   preset importer). Tint was never calibrated to a colorimetric unit. The
   adjustment blob is `serde_json::Value` on the backend and `any` on the
   frontend, unversioned.
8. **The WB eyedropper is a heuristic.** It reads screen-space sRGB pixels and
   applies two hardcoded scale factors (125 and 400) to guess a delta.
9. **Screenshot evaluation.** Downscale both to the same size, sample boxes on
   neutrals, compare R/G and B/G ratios, and confirm which path ran from the
   log. Lightroom shows an "Embedded Preview" badge when it's displaying the
   camera JPEG rather than its own render; still a usable reference, label it
   as the camera's rendering. Lanterns and anything clipped in red tell you
   nothing about the matrix, only about clip handling.
10. **On the lantern scene**, seven patches of door and wall: an sRGB-assuming
    path renders olive (R/G ≈ 1.5) where the camera JPEG has R/G ≈ 1.9. A
    color-managed path lands closer to the camera on all seven and within a
    few percent on five, and looks "very red" only because the scene really is
    lantern-orange relative to the 2986 K white point — the olive render was
    hiding that behind green.
11. **Every converter derives a different Kelvin** for the same file. The
    number is consistent within an app and across cameras; it will not match
    Lightroom's readout. Say so in the UI.
12. **Test at the host boundary.** Every bug worth the name in integrating a
    color crate with a host was a mismatch at that boundary (working
    primaries, tint sign, highlight roll-off), not inside the crate.
13. **Environment.** WebKitGTK on NVIDIA + Wayland crashes with
    `Error 71 (Protocol error)`; workaround `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
    The shell here is fish: `$VAR` does not word-split, `PIPESTATUS` doesn't
    exist, `npx` prints notices to stdout (use `./node_modules/.bin/eslint`),
    and `grep` is ugrep (use `-E`, no `\|`).

---
