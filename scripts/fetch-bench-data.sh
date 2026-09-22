#!/bin/sh
# Fetch the demosaic benchmark reference sets into data/bench/.
#
#   Kodak    24 images, 768x512, PNG   http://r0k.us/graphics/kodak/
#   McMaster 18 images, 500x500, TIF   https://www4.comp.polyu.edu.hk/~cslzhang/CDM_Dataset.htm
#
# Both are the standard sets demosaic papers report PSNR on. data/ is
# gitignored; run this once, then `cargo run -p greycard-bench --release -- data/bench/kodak data/bench/mcm`.
set -eu
cd "$(dirname "$0")/.."
mkdir -p data/bench/kodak data/bench/mcm

for i in $(seq -w 1 24); do
  f="data/bench/kodak/kodim$i.png"
  [ -s "$f" ] || curl -sSL --fail -o "$f" "http://r0k.us/graphics/kodak/kodak/kodim$i.png"
done

if [ -z "$(ls data/bench/mcm 2>/dev/null)" ]; then
  curl -sSL --fail -o data/bench/McM.zip "https://www4.comp.polyu.edu.hk/~cslzhang/DATA/McM.zip"
  # The archive is encrypted; the password is printed on the dataset page.
  (cd data/bench/mcm && unzip -q -P McM_CDM -j ../McM.zip 'McM/*.tif')
  rm data/bench/McM.zip
fi
echo "kodak: $(ls data/bench/kodak | wc -l) files, mcm: $(ls data/bench/mcm | wc -l) files"
