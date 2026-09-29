# 1. Where things stand

The engine starts from one conclusion about color: that white balance
applied in the wrong space, a camera matrix used without luminance
normalization, and a pipeline with many disagreeing render paths are
faults no patch fixes one at a time. §3 collects the rules that follow
from it, and they are the first design rules here.

`rawcolor` on crates.io carries the colorimetric half — CIE xy / CCT /
Duv, mired interpolation of DNG dual-illuminant matrices, Bradford
adaptation, working-space matrices for sRGB, Rec.2020 and ProPhoto.
Zero dependencies, `#![forbid(unsafe_code)]`, MIT OR Apache-2.0,
MSRV 1.75.

---
