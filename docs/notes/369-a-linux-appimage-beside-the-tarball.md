# §369. A Linux AppImage beside the tarball (2026-10-18)

A tester asked for an AppImage, and it costs little: the Linux
tarball is already most of an AppDir, bin/ with the two binaries and
the two libraries, share/ with the desktop entry and the icon. The
script lays that out as usr/, puts the desktop file, the icon, and an
AppRun at the root, and runs appimagetool over it, pinned to a
numbered release by checksum with the type 2 runtime pinned the same
way, so a build never fetches a moving target. The AppRun runs the
editor; `--cli` as the first argument runs the command line. The name
the image was started by never picks the CLI, since renaming an
AppImage to the program's name is the usual thing to do with one and
would otherwise hand a tester the command line's usage. The release job builds it on the
Linux runner after the tarball and attaches it as a fourth asset.

Nothing is bundled. glibc and the loader are the host's by
definition, so the floor stays the tarball's, 2.39, and the AppImage
runs nowhere the tarball would not. libgcc_s and libstdc++ are on
every desktop and the GPU driver loads its own copies into the same
process, so a bundled pair would only conflict. fontconfig must match
the host's configuration and cache, and freetype, libpng, brotli,
expat, bz2, and zlib come in through it. Vulkan, Wayland, xkbcommon,
and X11 are opened by name at run time, and a bundled Vulkan loader
would not find the host's drivers. The one library the binary linked
that a bare desktop may lack, lcms2, is now built in statically on
Linux as it was on the other platforms, which takes it off the
tarball's needs too. A check in the script, per file, fails the
build if any other library turns up in a binary's needs, and fails
rather than passes when readelf is missing.

The image is reproducible, two builds byte for byte the same, from a
fixed timestamp. It needs the kernel's FUSE to mount, not libfuse2,
since the runtime is static; `--appimage-extract-and-run` is the way
around that. Settings, cache, and models are shared with an installed
copy, and the launcher's own integration, or none, decides whether
there is a menu entry.
