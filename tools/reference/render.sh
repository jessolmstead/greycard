#!/usr/bin/env bash
# Render one variant of a reference frame through the editor's own
# export, one tone slider set and everything else at the default (notes
# §143). The raw is copied into WORK first and its sidecar written
# there, so the frame's real sidecar is never read or touched.
#
#     tools/reference/render.sh RAW WORK VARIANT [FIELD VALUE]
#     EXP=1 tools/reference/render.sh .../4Z4A3521.CR3 work base_plusone
#     tools/reference/render.sh .../4Z4A2623.CR3 work whites_up whites 2
#
# Writes WORK/out_<stem>_<VARIANT>.jpg. FIELD is one of highlights,
# shadows, whites, blacks, contrast; EXP is the exposure slider (0).
# Lens corrections are on, as the Lightroom side has them. Build with
# `cargo build --release -p greycard-ui` first: a debug build takes
# minutes a frame where the release one takes about twelve seconds.
set -euo pipefail

raw=$1 work=$2 variant=$3 field=${4:-} value=${5:-}
here=$(cd "$(dirname "$0")/../.." && pwd)
exe=$here/target/release/greycard-ui
[ -x "$exe" ] || exe=$exe.exe

mkdir -p "$work"
name=$(basename "$raw")
stem=${name%.*}
copy=$work/$name
[ -f "$copy" ] || cp "$raw" "$copy"

tone='"contrast":1.0,"highlights":0,"shadows":0,"whites":0,"blacks":0'
if [ -n "$field" ]; then
    tone=$(echo "$tone" | sed "s/\"$field\":[^,]*/\"$field\":$value/")
fi
cat > "$copy.gcd" <<JSON
{"current":{"version":4,
 "light":{"enabled":true,"exposure":${EXP:-0},"tone":{"enabled":true,$tone}},
 "lens":{"enabled":true,"profile":true,"distortion":true,"chromatic_aberration":true,
  "vignetting":true,"manual":0,"ca_red":0,"ca_blue":0,"auto_scale":true,"scale":1,
  "defringe":false,"defringe_radius":2,"defringe_threshold":13,
  "defringe_purple_center":310,"defringe_purple_width":120,"defringe_purple_amount":1,
  "defringe_green_center":130,"defringe_green_width":120,"defringe_green_amount":1}},
 "history":[],"snapshots":[]}
JSON

out=$work/out_${stem}_$variant.jpg
rm -f "$out"
"$exe" --no-display-profile --export "$out" "$copy" > /dev/null 2>&1
echo "$out"
