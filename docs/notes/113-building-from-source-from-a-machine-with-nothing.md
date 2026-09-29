# 113. Building from source, from a machine with nothing on it (2026-09-20)

The README's *From source* paragraph told you the package names and
the cargo line, which is enough for someone who already has a
toolchain and knows what a toolchain is. It is not enough for the
people the project wants: a photographer on a distribution the
tarball does not suit, or anyone who wants a fix before it is
released. `docs/building.md` is that walk, in five steps — the
compiler and the libraries, rustup, the clone, the build, running it
— with Debian, Fedora and Arch lines and `xcode-select --install`
for the Mac, and the README now points at it the way it points at
the tester's guide (§111).

The parts that are worth writing down because they are not
guessable: `libwebgpu_dawn.so` in `target/release` is a symlink into
`~/.cache/ort.pyke.io`, so clearing that cache breaks an already
built binary and `cargo clean -p greycard-ai` is the fix; git is
needed to *build*, not only to clone, while rawler is pinned to a
revision (§13j); and the `#[ignore]`d tests want `GREYCARD_MODELS`
and `GREYCARD_SAMPLES` or they pass in no time and say nothing, the
trap §58 already recorded once. The troubleshooting section is keyed
to the actual failures — no linker, no lcms2, an MSRV below 1.98, no
Vulkan adapter, the dangling Dawn link, and the OOM kill that `-j 2`
gets past — rather than to imagined ones.

Two numbers in it are estimates and marked as nothing better: ten to
twenty minutes for a first release build, and around ten gigabytes
under `target/` for a debug and a release build together.
