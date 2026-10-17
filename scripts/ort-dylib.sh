#!/bin/sh
# Make the ONNX Runtime shared library greycard loads at first use, and
# put it with Dawn beside the binaries of each profile named.
#
#     scripts/ort-dylib.sh [profile ...]      (debug and release if none)
#
# The editor and the CLI no longer link ONNX Runtime: pyke's prebuilt
# one is compiled for x86-64-v3 and its initializers ran before main,
# so a processor without AVX2 or BMI2 died at exec. greycard-ai loads
# it the first time a model is wanted, after asking the processor,
# from target/<profile>/ in a build and from beside the executable in
# a package. pyke publishes only a static archive, so this links one
# shared library out of it: libonnxruntime.so on Linux,
# libonnxruntime.dylib on macOS, onnxruntime.dll on Windows. The work
# is done by the ort-dylib crate (crates/ort-dylib/src/main.rs, whose
# header has the recipe for each platform), since unpacking pyke's
# raw LZMA2 archive and finding MSVC's link.exe are not things a
# shell does alike on three hosts.
#
# Run once after a fresh clone or a cargo clean, and again when ort is
# bumped; a second run with nothing changed fetches and links nothing.
# It needs a C++ toolchain: c++ (or $CXX) on Linux and macOS, the
# Visual Studio C++ build tools on Windows. The download goes to
# target/ort-dylib/, never to a cache outside the tree. A build without
# the library runs, with the AI features off and a line saying why.
#
# CARGO_TARGET_DIR is honored, as cargo honors it.

set -eu

cd "$(dirname "$0")/.."

target=${CARGO_TARGET_DIR:-target}
exec cargo run --locked --quiet -p ort-dylib -- --target-dir "$target" "$@"
