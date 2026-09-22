#!/bin/sh
# Build the release tarball from an existing release build.
#
#     cargo build --release -p greycard-ui -p greycard-cli
#     scripts/package.sh [version]
#
# The version comes from cargo metadata; the release workflow passes the
# tag with its leading v stripped, and an argument that disagrees with
# the workspace is an error rather than a mislabelled tarball. The result
# is target/dist/greycard-<version>-<arch>-<os>.tar.gz, one per host it
# is rolled on, and each is laid out so that unpacking it and running
# install.sh puts everything where that desktop expects it:
#
#   Linux    bin/ with the two binaries and the library, share/ with
#            the desktop entry and the icon.
#   macOS    greycard.app, a bundle with both binaries and the library
#            in Contents/MacOS and the icon in Contents/Resources, so
#            it can go into Applications and be opened like any app.
#            install.sh links the command line out of it.
#   Windows  bin/ like Linux, with no share/. An installer is still to
#            do.
#
# The one library we ship is Dawn: libwebgpu_dawn.so on Linux, .dylib
# on macOS, webgpu_dawn.dll on Windows. greycard-ai links ONNX Runtime
# statically but Dawn comes as a shared object, and the build leaves a
# symlink to it in target/release; it is copied dereferenced so the
# tarball stands alone. The binaries carry $ORIGIN (@executable_path on
# macOS) on their runpath, so beside them is where it has to land.
set -eu

cd "$(dirname "$0")/.."

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
    echo "package.sh: cargo metadata has no version for greycard-ui" >&2
    exit 1
fi

# An argument has to agree with what was built. Otherwise a tag could
# put its own number on a tarball whose binaries print another one.
version=${1:-$crate}
if [ "$version" != "$crate" ]; then
    echo "package.sh: asked to package $version, but the workspace is at" \
        "$crate." >&2
    echo "  Bump the crate versions to match the tag, or drop the" \
        "argument." >&2
    exit 1
fi

# The host's triple names the tarball and the shared library.
host=$(rustc -vV | sed -n 's/^host: //p')
arch=${host%%-*}
case "$host" in
*-linux-*) os=linux; dawn=libwebgpu_dawn.so; exe= ;;
*-darwin*) os=macos; dawn=libwebgpu_dawn.dylib; exe= ;;
*-windows-*) os=windows; dawn=webgpu_dawn.dll; exe=.exe ;;
*)
    echo "package.sh: no tarball layout for host $host" >&2
    exit 1
    ;;
esac

name="greycard-$version-$arch-$os"
build=target/release
stage="target/dist/$name"

# -e follows the link, so a Dawn symlink left pointing at a download
# cache that has since been cleared fails here rather than silently
# shipping nothing.
for f in "$build/greycard-ui$exe" "$build/greycard$exe" "$build/$dawn"; do
    if [ ! -e "$f" ]; then
        echo "package.sh: $f is missing; run cargo build --release first" >&2
        exit 1
    fi
done

rm -rf "$stage" "target/dist/$name.tar" "target/dist/$name.tar.gz"
mkdir -p "$stage"

if [ "$os" = macos ]; then
    # The bundle. Both binaries sit in Contents/MacOS beside the one
    # copy of Dawn, since each finds it through @executable_path.
    app="$stage/greycard.app/Contents"
    mkdir -p "$app/MacOS" "$app/Resources"
    cp "$build/greycard-ui" "$build/greycard" "$app/MacOS/"
    cp -L "$build/$dawn" "$app/MacOS/"
    chmod 0755 "$app/MacOS/greycard-ui" "$app/MacOS/greycard"
    chmod 0644 "$app/MacOS/$dawn"
    sed "s/@VERSION@/$version/g" packaging/Info.plist > "$app/Info.plist"

    # The icon, rasterized from the SVG by sips at each size an icns
    # holds, and folded by iconutil. Both ship with macOS.
    iconset="target/dist/greycard.iconset"
    rm -rf "$iconset"
    mkdir -p "$iconset"
    for px in 16 32 128 256 512; do
        sips -s format png -z "$px" "$px" assets/icon/greycard.svg \
            --out "$iconset/icon_${px}x${px}.png" >/dev/null
        double=$((px * 2))
        sips -s format png -z "$double" "$double" assets/icon/greycard.svg \
            --out "$iconset/icon_${px}x${px}@2x.png" >/dev/null
    done
    iconutil -c icns "$iconset" -o "$app/Resources/greycard.icns"
    rm -rf "$iconset"

    # An ad-hoc signature over the whole bundle. The linker already
    # signed each binary that way, which Apple silicon insists on; this
    # seals Info.plist and the resources with them so the bundle is one
    # signed thing and not three. It is not a Developer ID: a bundle
    # downloaded by a browser still arrives quarantined, and install.sh
    # clears that.
    codesign --force --deep --sign - "$stage/greycard.app"
else
    mkdir -p "$stage/bin"
    cp "$build/greycard-ui$exe" "$build/greycard$exe" "$stage/bin/"
    cp -L "$build/$dawn" "$stage/bin/"
    chmod 0755 "$stage/bin/greycard-ui$exe" "$stage/bin/greycard$exe"
    chmod 0644 "$stage/bin/$dawn"
    if [ "$os" = linux ]; then
        mkdir -p "$stage/share/applications" \
            "$stage/share/icons/hicolor/scalable/apps" \
            "$stage/share/icons/hicolor/scalable/mimetypes" \
            "$stage/share/mime/packages"
        cp assets/greycard.desktop "$stage/share/applications/"
        cp assets/icon/greycard.svg "$stage/share/icons/hicolor/scalable/apps/"
        cp assets/icon/application-x-greycard-edit.svg "$stage/share/icons/hicolor/scalable/mimetypes/"
        cp packaging/linux/greycard.xml "$stage/share/mime/packages/"
    fi
fi

cp packaging/install.sh packaging/uninstall.sh "$stage/"
chmod 0755 "$stage/install.sh" "$stage/uninstall.sh"
cp LICENSE README.md "$stage/"

# Anything the binaries want beyond the base system a desktop already
# has would be a surprise at unpack time, so print the list into the
# build log where it can be read after the fact.
if [ "$os" = macos ]; then
    echo "shared libraries:"
    otool -L "$app/MacOS/greycard-ui" | sed 's/^/  ui  /'
    otool -L "$app/MacOS/greycard" | sed 's/^/  cli /'
elif command -v ldd >/dev/null 2>&1; then
    echo "shared libraries:"
    ldd "$stage/bin/greycard-ui$exe" | sed 's/^/  ui  /'
    ldd "$stage/bin/greycard$exe" | sed 's/^/  cli /'
fi

# Nothing about who rolled this or when belongs in the archive: names
# sorted, owner root, and the commit's own date on every entry, so two
# builds of the same commit come out byte for byte the same. GNU tar
# has flags for all of that and bsdtar, which macOS ships, has
# different ones for some and none for the rest, so the parts both can
# do are done the same way: the entries come from a sorted list rather
# than a directory walk, and the date is put on the files themselves
# rather than passed to tar. ustar is the one format both write
# without extended headers, and gzip -n keeps the timestamp out of the
# compressed header too.
mtime=$(TZ=UTC git log -1 --date=format-local:%Y%m%d%H%M.%S --format=%cd \
    2>/dev/null) || mtime=
[ -n "$mtime" ] || mtime=197001010000.00
if [ "$os" = macos ]; then
    # Extended attributes would come along as ._ entries otherwise.
    xattr -cr "$stage"
fi
find "$stage" -exec env TZ=UTC touch -t "$mtime" {} +
(cd target/dist && find "$name" | LC_ALL=C sort) > "target/dist/$name.list"
if tar --version 2>/dev/null | grep -q GNU; then
    tar --format=ustar --no-recursion --owner=0 --group=0 --numeric-owner \
        -cf "target/dist/$name.tar" -C target/dist -T "target/dist/$name.list"
else
    COPYFILE_DISABLE=1 tar --format ustar -n --uid 0 --gid 0 \
        --uname '' --gname '' --no-xattrs --no-mac-metadata \
        -cf "target/dist/$name.tar" -C target/dist -T "target/dist/$name.list"
fi
rm -f "target/dist/$name.list"
gzip -9nf "target/dist/$name.tar"
echo "target/dist/$name.tar.gz"
