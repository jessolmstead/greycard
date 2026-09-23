#!/bin/sh
# Build the release package from an existing release build.
#
#     cargo build --release -p greycard-ui -p greycard-cli
#     scripts/package.sh [version]
#
# The version comes from cargo metadata; the release workflow passes the
# tag with its leading v stripped, and an argument that disagrees with
# the workspace is an error rather than a mislabelled package. The
# result is target/dist/greycard-<version>-<arch>-<os>.tar.gz, or
# .zip on Windows, one per host it is rolled on, and each is laid
# out so that unpacking it and running the installer beside it puts
# everything where that desktop expects it:
#
#   Linux    bin/ with the two binaries and the library, share/ with
#            the desktop entry and the icon.
#   macOS    greycard.app, a bundle with both binaries and the library
#            in Contents/MacOS and the icons in Contents/Resources, so
#            it can go into Applications and be opened like any app.
#            install.sh links the command line out of it. The sidecar
#            gets its own .icns beside the app's, for Info.plist's
#            document types.
#   Windows  bin/ with the two binaries, the three DLLs and the
#            sidecar icon; install.cmd, uninstall.cmd, register.cmd
#            and unregister.cmd beside it rather than install.sh.
#            The archive is a .zip: Windows 10's Explorer opens one
#            by double-click and does not open a .tar.gz at all.
#
# The libraries we ship are Dawn and, on Windows, the two DXC ones it
# loads by name. greycard-ai links ONNX Runtime statically, but Dawn
# comes as a shared object: libwebgpu_dawn.so on Linux, .dylib on
# macOS, webgpu_dawn.dll on Windows. On Windows that DLL calls
# LoadLibrary on dxcompiler.dll and dxil.dll to compile shaders for
# the D3D12 backend, and ort's prebuilt bundle puts all three in
# target/release; shipping only Dawn leaves a package whose viewport
# fails on the machine it is unpacked on. The build leaves symlinks
# to them there on a host that can make one, so they are copied
# dereferenced and the archive stands alone. The binaries carry
# $ORIGIN (@executable_path on macOS) on their runpath, so beside
# them is where it has to land; Windows looks beside the executable
# on its own.

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

# The host's triple names the package and the shared libraries.
host=$(rustc -vV | sed -n 's/^host: //p')
arch=${host%%-*}
case "$host" in
*-linux-*) os=linux; libs=libwebgpu_dawn.so; exe= ;;
*-darwin*) os=macos; libs=libwebgpu_dawn.dylib; exe= ;;
# Dawn loads the two DXC libraries by name at runtime; see above.
*-windows-*) os=windows; libs="webgpu_dawn.dll dxcompiler.dll dxil.dll"; exe=.exe ;;
*)
    echo "package.sh: no package layout for host $host" >&2
    exit 1
    ;;
esac

name="greycard-$version-$arch-$os"
build=target/release
stage="target/dist/$name"

# -e follows the link, so a library symlink left pointing at a
# download cache that has since been cleared fails here rather than
# silently shipping nothing.
want="$build/greycard-ui$exe $build/greycard$exe"
for l in $libs; do want="$want $build/$l"; done
for f in $want; do
    if [ ! -e "$f" ]; then
        echo "package.sh: $f is missing; run cargo build --release first" >&2
        exit 1
    fi
done

rm -rf "$stage" "target/dist/$name.tar" "target/dist/$name.tar.gz" \
    "target/dist/$name.zip"
mkdir -p "$stage"

if [ "$os" = macos ]; then
    # The bundle. Both binaries sit in Contents/MacOS beside the one
    # copy of Dawn, since each finds it through @executable_path.
    app="$stage/greycard.app/Contents"
    mkdir -p "$app/MacOS" "$app/Resources"
    cp "$build/greycard-ui" "$build/greycard" "$app/MacOS/"
    for l in $libs; do cp -L "$build/$l" "$app/MacOS/"; done
    chmod 0755 "$app/MacOS/greycard-ui" "$app/MacOS/greycard"
    for l in $libs; do chmod 0644 "$app/MacOS/$l"; done
    sed "s/@VERSION@/$version/g" packaging/Info.plist > "$app/Info.plist"

    # The icon, rasterized from the SVG by sips at each size an icns
    # holds, and folded by iconutil. Both ship with macOS. The sidecar
    # gets the same treatment from its own SVG, so Info.plist's .gcd
    # document type has an icon of its own rather than the app's —
    # the same two-cards-on-a-document mark Linux and Windows use for
    # it.
    for icon in greycard application-x-greycard-edit; do
        iconset="target/dist/$icon.iconset"
        rm -rf "$iconset"
        mkdir -p "$iconset"
        for px in 16 32 128 256 512; do
            sips -s format png -z "$px" "$px" "assets/icon/$icon.svg" \
                --out "$iconset/icon_${px}x${px}.png" >/dev/null
            double=$((px * 2))
            sips -s format png -z "$double" "$double" "assets/icon/$icon.svg" \
                --out "$iconset/icon_${px}x${px}@2x.png" >/dev/null
        done
        iconutil -c icns "$iconset" -o "$app/Resources/$icon.icns"
        rm -rf "$iconset"
    done

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
    for l in $libs; do cp -L "$build/$l" "$stage/bin/"; done
    chmod 0755 "$stage/bin/greycard-ui$exe" "$stage/bin/greycard$exe"
    for l in $libs; do chmod 0644 "$stage/bin/$l"; done
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
    if [ "$os" = windows ]; then
        # The sidecar's icon is the one .ico that has to be on disk:
        # register.cmd points the .gcd document type at it, while the
        # app's own icon is read out of greycard-ui.exe, which carries
        # it as a resource. It sits in bin/ beside the binaries so the
        # installer moves one directory and register.cmd finds it
        # whether it is run from the unpacked archive or the installed
        # copy.
        cp assets/icon/application-x-greycard-edit.ico "$stage/bin/"

        # The Visual C++ runtime, beside the binaries. Windows ships
        # the UCRT but not this: a machine that has never had a Visual
        # Studio redistributable on it answers greycard-ui.exe with a
        # "VCRUNTIME140.dll was not found" box and no other clue, and
        # the redistributable is common enough that the machine it was
        # built on will never show that. The five are what the two
        # binaries and Dawn import between them, and they are closed
        # under their own imports; app-local is a deployment
        # Microsoft's redistributable licence allows, and these come
        # from the toolset's own Redist directory rather than from
        # System32, which is the installed copy and not ours to hand
        # on. Set VCREDIST_DIR to point somewhere else, or
        # GREYCARD_SKIP_VCREDIST=1 to roll a package without them and
        # take the prerequisite back on.
        if [ -z "${GREYCARD_SKIP_VCREDIST:-}" ]; then
            vcredist=${VCREDIST_DIR:-}
            if [ -z "$vcredist" ]; then
                vswhere="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
                if [ -x "$vswhere" ]; then
                    vsroot=$("$vswhere" -latest -products '*' \
                        -property installationPath 2>/dev/null | tr -d '\r')
                    [ -n "$vsroot" ] && vsroot=$(cygpath -u "$vsroot" 2>/dev/null ||
                        printf '%s' "$vsroot")
                    # Last match wins: the versions sort as 14.NN, so
                    # that is the newest, which is the toolset rustc
                    # picked to link with.
                    for d in "$vsroot"/VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT; do
                        [ -d "$d" ] && vcredist=$d
                    done
                fi
            fi
            if [ -z "$vcredist" ] || [ ! -d "$vcredist" ]; then
                echo "package.sh: no Visual C++ redistributable directory found." >&2
                echo "  Set VCREDIST_DIR to a Microsoft.VC*.CRT folder, or" >&2
                echo "  GREYCARD_SKIP_VCREDIST=1 to ship without it." >&2
                exit 1
            fi
            for l in msvcp140.dll msvcp140_1.dll msvcp140_atomic_wait.dll \
                vcruntime140.dll vcruntime140_1.dll; do
                if [ ! -f "$vcredist/$l" ]; then
                    echo "package.sh: $vcredist has no $l" >&2
                    exit 1
                fi
                cp "$vcredist/$l" "$stage/bin/"
                chmod 0644 "$stage/bin/$l"
            done
            echo "Visual C++ runtime from $vcredist"
        fi
    fi
fi

if [ "$os" = windows ]; then
    cp packaging/windows/install.cmd packaging/windows/uninstall.cmd \
        packaging/windows/install.ps1 packaging/windows/register.cmd \
        packaging/windows/unregister.cmd "$stage/"
else
    cp packaging/install.sh packaging/uninstall.sh "$stage/"
    chmod 0755 "$stage/install.sh" "$stage/uninstall.sh"
fi
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
# compressed header too. Windows gets a .zip instead, written by
# scripts/zip.ps1, which takes the same care for the same reason.
mtime=$(TZ=UTC git log -1 --date=format-local:%Y%m%d%H%M.%S --format=%cd \
    2>/dev/null) || mtime=
[ -n "$mtime" ] || mtime=197001010000.00
if [ "$os" = windows ]; then
    # A zip carries no ownership or permission bits to normalize, so
    # the date is all there is to pin, and it goes in as ISO 8601
    # rather than touch's format. Nothing is touched first: zip.ps1
    # stamps every entry itself and never reads a file's own mtime.
    iso=$(TZ=UTC git log -1 --date=format-local:%Y-%m-%dT%H:%M:%SZ \
        --format=%cd 2>/dev/null) || iso=
    # The floor a zip's MS-DOS timestamps can hold, for a tree with
    # no commits; zip.ps1 clamps to the same date.
    [ -n "$iso" ] || iso=1980-01-01T00:00:00Z
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/zip.ps1 \
        -Source "target/dist/$name" \
        -Destination "target/dist/$name.zip" -Date "$iso"
    echo "target/dist/$name.zip"
    exit 0
fi

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
