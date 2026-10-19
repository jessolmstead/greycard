#!/bin/sh
# Build the Linux AppImage from the package scripts/package.sh rolled.
#
#     cargo build --release -p greycard-ui -p greycard-cli
#     scripts/ort-dylib.sh release
#     scripts/package.sh [version]
#     scripts/appimage.sh [version]
#
# package.sh leaves its staging directory beside the tarball, and this
# takes that, so the AppImage carries exactly what the tarball does.
# The version comes from cargo metadata as package.sh has it, and an
# argument that disagrees is an error for the same reason. The result
# is target/dist/greycard-<version>-linux-x86_64.AppImage, on an
# x86_64 Linux host only; its AppDir is left beside it.
#
# The AppDir:
#
#   AppRun               packaging/linux/AppRun: runs usr/bin/greycard-ui,
#                        or usr/bin/greycard with --cli first
#   greycard.desktop     the tarball's desktop entry, with the version
#   greycard.svg         the icon the entry's Icon= names
#   usr/bin/             the two binaries, libonnxruntime.so and
#                        libwebgpu_dawn.so side by side, as in the
#                        tarball's bin/: both binaries look for ONNX
#                        Runtime beside themselves and it finds Dawn
#                        through $ORIGIN
#   usr/share/           the tarball's share/, and the license and
#                        README under doc/greycard
#
# What is not bundled, and why. The binaries link, at exec, glibc
# (libc, libm, and ld-linux), libgcc_s, and, for greycard-ui,
# fontconfig, and nothing else; ONNX Runtime and Dawn add libstdc++,
# and ONNX Runtime needs Dawn, which sits beside it. lcms2 is built in
# statically (LCMS2_STATIC, the release workflow's "Static lcms2"
# step), so no host is asked for it.
# glibc and the loader are the host's by definition; libgcc_s and
# libstdc++ are on every desktop that has a compiled program on it,
# and a copy of ours would fight the host's driver stack, which loads
# the host's into the same process. fontconfig has to be the host's,
# since its configuration and cache are, and an older bundled one
# reads a newer system's config badly; freetype, expat, libpng,
# brotli, bzip2, and zlib come in under it and so are the host's for
# the same reason. Vulkan, Wayland, xkbcommon, and X11 are opened at
# run time by name and are the host's driver and session; a bundled
# loader would not find the host's drivers. So there is no usr/lib,
# AppRun sets no LD_LIBRARY_PATH, and the check below fails the build
# if a file ever needs anything outside its share of that list. The
# glibc floor
# is the build host's, the same as the tarball's (see the head of
# .github/workflows/release.yml).
#
# appimagetool and the AppImage runtime it embeds are fetched once
# into target/appimagetool/, each pinned to a numbered release and
# checked against its SHA-256; a file already there that matches is
# not fetched again. Without --runtime-file appimagetool would fetch
# the runtime's continuous build itself, at whatever it is that day.
# appimagetool runs with --appimage-extract-and-run, so the host needs
# no FUSE. The runtime is the static type 2 one, so the AppImage
# needs FUSE on the machine it runs on (the kernel module and
# fusermount) but not libfuse2.

set -eu

cd "$(dirname "$0")/.."

appimagetool_url=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
appimagetool_sha256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
runtime_url=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
runtime_sha256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

host=$(rustc -vV | sed -n 's/^host: //p')
case "$host" in
x86_64-*-linux-*) ;;
*)
    echo "appimage.sh: the AppImage is x86_64 Linux only; this host is $host" >&2
    exit 1
    ;;
esac

meta=$(cargo metadata --format-version 1 --no-deps)
if command -v jq >/dev/null 2>&1; then
    crate=$(printf '%s' "$meta" |
        jq -r '.packages[] | select(.name == "greycard-ui") | .version')
else
    crate=$(printf '%s' "$meta" | tr '{' '\n' |
        sed -n 's/.*"name":"greycard-ui","version":"\([^"]*\)".*/\1/p' |
        head -n 1)
fi
if [ -z "$crate" ]; then
    echo "appimage.sh: cargo metadata has no version for greycard-ui" >&2
    exit 1
fi
version=${1:-$crate}
if [ "$version" != "$crate" ]; then
    echo "appimage.sh: asked for $version, but the workspace is at $crate." >&2
    exit 1
fi

stage="target/dist/greycard-$version-x86_64-linux"
name="greycard-$version-linux-x86_64"
appdir="target/dist/$name.AppDir"
out="target/dist/$name.AppImage"
tools=target/appimagetool

if ! command -v readelf >/dev/null 2>&1; then
    echo "appimage.sh: readelf is missing; it comes with binutils" >&2
    exit 1
fi
for f in "$stage/bin/greycard-ui" "$stage/bin/greycard" \
    "$stage/bin/libonnxruntime.so" "$stage/bin/libwebgpu_dawn.so" \
    "$stage/share/applications/greycard.desktop" \
    "$stage/share/icons/hicolor/scalable/apps/greycard.svg"; do
    if [ ! -f "$f" ]; then
        echo "appimage.sh: $f is missing; run scripts/package.sh first" >&2
        exit 1
    fi
done

# fetch <url> <sha256> <file>: the pinned file, checked.
fetch() {
    if [ -f "$3" ] && [ "$(sha256sum "$3" | cut -d' ' -f1)" = "$2" ]; then
        return
    fi
    mkdir -p "$(dirname "$3")"
    rm -f "$3" "$3.part"
    curl --fail --location --silent --show-error --retry 3 --max-time 600 \
        -o "$3.part" "$1"
    got=$(sha256sum "$3.part" | cut -d' ' -f1)
    if [ "$got" != "$2" ]; then
        rm -f "$3.part"
        echo "appimage.sh: $1 has SHA-256 $got, not the pinned $2" >&2
        exit 1
    fi
    mv "$3.part" "$3"
}
fetch "$appimagetool_url" "$appimagetool_sha256" "$tools/appimagetool-1.9.1-x86_64.AppImage"
fetch "$runtime_url" "$runtime_sha256" "$tools/runtime-20251108-x86_64"
chmod 0755 "$tools/appimagetool-1.9.1-x86_64.AppImage"

rm -rf "$appdir" "$out"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/doc/greycard"
cp "$stage/bin/greycard-ui" "$stage/bin/greycard" \
    "$stage/bin/libonnxruntime.so" "$stage/bin/libwebgpu_dawn.so" \
    "$appdir/usr/bin/"
cp -R "$stage/share/." "$appdir/usr/share/"
cp "$stage/LICENSE" "$stage/README.md" "$appdir/usr/share/doc/greycard/"
cp packaging/linux/AppRun "$appdir/AppRun"
chmod 0755 "$appdir/AppRun" "$appdir/usr/bin/greycard-ui" "$appdir/usr/bin/greycard"
chmod 0644 "$appdir/usr/bin/libonnxruntime.so" "$appdir/usr/bin/libwebgpu_dawn.so"

# appimagetool wants the entry and the icon it names at the top. The
# entry's Exec= stays greycard-ui, the binary's name, which is what a
# launcher that integrates AppImages rewrites to the AppImage's path.
sed "/^\[Desktop Entry\]$/a X-AppImage-Version=$version" \
    "$stage/share/applications/greycard.desktop" >"$appdir/greycard.desktop"
cp "$stage/share/icons/hicolor/scalable/apps/greycard.svg" "$appdir/greycard.svg"
ln -s greycard.svg "$appdir/.DirIcon"

# What each file loads at exec, against its share of the list above.
# allowed <file> <library>: whether that file may ask the host for it.
allowed() {
    case "$2" in
    libc.so.6 | libm.so.6 | ld-linux-x86-64.so.2 | libgcc_s.so.1) return 0 ;;
    esac
    case "$1:$2" in
    greycard-ui:libfontconfig.so.1) return 0 ;;
    libonnxruntime.so:libstdc++.so.6 | libwebgpu_dawn.so:libstdc++.so.6) return 0 ;;
    libonnxruntime.so:libwebgpu_dawn.so) return 0 ;;
    esac
    return 1
}
echo "shared libraries:"
stray=
for f in "$appdir/usr/bin/greycard-ui" "$appdir/usr/bin/greycard" \
    "$appdir/usr/bin/libonnxruntime.so" "$appdir/usr/bin/libwebgpu_dawn.so"; do
    # Read first and parsed after: a readelf that fails inside the for
    # list below would leave it empty and the check passed.
    dynamic=$(readelf -d "$f")
    libs=$(printf '%s\n' "$dynamic" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
    # Every one of them needs libc, so a list without it was not read.
    if ! printf '%s\n' "$libs" | grep -qx 'libc\.so\.6'; then
        echo "appimage.sh: readelf lists no libc.so.6 for $f" >&2
        exit 1
    fi
    for lib in $libs; do
        echo "  ${f##*/}  $lib"
        allowed "${f##*/}" "$lib" || stray="$stray ${f##*/}:$lib"
    done
done
if [ -n "$stray" ]; then
    echo "appimage.sh: the AppImage would ask the host for libraries it" \
        "may not have:" >&2
    # Split on purpose: one line a library.
    # shellcheck disable=SC2086
    printf '  %s\n' $stray >&2
    exit 1
fi

# The commit's date on every entry and in the image's own header, as
# package.sh does for the tarball, so the build says nothing about when
# it was rolled.
SOURCE_DATE_EPOCH=$(git log -1 --format=%ct 2>/dev/null) || SOURCE_DATE_EPOCH=
[ -n "$SOURCE_DATE_EPOCH" ] || SOURCE_DATE_EPOCH=0
export SOURCE_DATE_EPOCH
find "$appdir" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +

ARCH=x86_64 "$tools/appimagetool-1.9.1-x86_64.AppImage" --appimage-extract-and-run \
    --no-appstream --runtime-file "$tools/runtime-20251108-x86_64" \
    "$appdir" "$out"
echo "$out"
