#!/bin/sh
# The Windows package's side of signing through SignPath, on either
# side of the first signing request; the release workflow's "Windows
# signing" step has the whole of it.
#
#     scripts/signpath.sh request [version]
#     scripts/signpath.sh package [version]
#
# request runs after scripts/package.sh has staged and zipped the
# package unsigned, with Inno Setup 7 at hand. It fills
# target/signpath/request with what the first request signs: the two
# executables from the staging directory, and Inno Setup's uninstaller
# as uninstaller.exe. Inno makes the uninstaller at compile time and
# signs it only through a sign tool it runs itself, and SignPath signs
# what a workflow hands it, not what a compiler asks for. So the
# installer is compiled once here in Inno's signed-uninstaller mode
# with nothing signed yet: it writes the uninstaller to
# target/signpath/uninstaller as uninst-<inno version>-<hash>.e32 and
# stops, asking for it to be signed. The unsigned zip is kept aside as
# target/signpath/unsigned.zip.
#
# package runs once the workflow has the request back signed in
# target/signpath/signed. It puts the signed uninstaller back under
# its own name, where the installer's second compile
# (scripts/windows-setup.ps1 -SignedUninstallerDir) finds it and puts
# its signature on the uninstaller and on Setup's own temporary copies.
# It rolls the package again with the signed executables in it, and
# checks that the new zip differs from the unsigned one in those two
# files and nowhere else. Our own executables are all we sign: the
# DLLs beside them are Microsoft's and ONNX Runtime's and Dawn's,
# which SignPath Foundation's terms keep out of our signature.
#
# The version, as package.sh takes it; Windows alone.

set -eu

cd "$(dirname "$0")/.."

phase=${1:-}
version=${2:-}
# The workspace's, as the release's version job reads it, with the CR
# a Windows checkout may leave on the line taken off first.
if [ -z "$version" ]; then
    version=$(tr -d '\r' < Cargo.toml | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n 1)
fi
if [ -z "$version" ]; then
    echo "signpath.sh: no version given, and none in Cargo.toml" >&2
    exit 1
fi
# Inno Setup pads the version in its files' version resources to 50
# characters, and SignPath's check spells the padding out
# (packaging/signpath). A version of another length would fail at
# SignPath after the whole build; it fails here first.
padded=$(printf '%s%*s' "$version" $((50 - ${#version})) '')
for f in packaging/signpath/windows-binaries.xml packaging/signpath/windows-setup.xml; do
    if ! grep -qF "product-version=\"\${version}${padded#"$version"}\"" "$f"; then
        echo "signpath.sh: $f does not pad a version as long as $version to 50 characters;" >&2
        echo "change the spaces there and in SignPath's copy" >&2
        exit 1
    fi
done
host=$(rustc -vV | sed -n 's/^host: //p')
name="greycard-$version-${host%%-*}-windows"
work=target/signpath

uninstaller() {
    set -- "$work"/uninstaller/uninst-*.e32 "$work"/uninstaller/uninst-*.e64
    found=
    for f in "$@"; do
        [ -f "$f" ] || continue
        if [ -n "$found" ]; then
            echo "signpath.sh: more than one uninstaller in $work/uninstaller" >&2
            exit 1
        fi
        found=$f
    done
    if [ -z "$found" ]; then
        echo "signpath.sh: Inno Setup wrote no uninstaller to $work/uninstaller" >&2
        exit 1
    fi
    printf '%s\n' "$found"
}

case "$phase" in
request)
    rm -rf "$work"
    mkdir -p "$work/request" "$work/uninstaller"
    mv "target/dist/$name.zip" "$work/unsigned.zip"
    cp "target/dist/$name/bin/greycard-ui.exe" "target/dist/$name/bin/greycard.exe" \
        "$work/request/"
    # Stopping is this compile's whole job, so its failure is expected;
    # what has to be there afterwards is the uninstaller. Its output is
    # shown when that is missing, since then it failed for some other
    # reason, which is the one worth reading.
    echo "compiling the installer once for its uninstaller; Inno Setup stops by design"
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/windows-setup.ps1 \
        -Version "$version" -SignedUninstallerDir "$work/uninstaller" \
        > "$work/first-compile.log" 2>&1 || true
    if ! u=$(uninstaller); then
        echo "signpath.sh: the first compile said:" >&2
        cat "$work/first-compile.log" >&2
        exit 1
    fi
    tail -n 5 "$work/first-compile.log"
    cp "$u" "$work/request/uninstaller.exe"
    ls -l "$work/request"
    ;;
package)
    u=$(uninstaller)
    cp "$work/signed/uninstaller.exe" "$u"
    WINDOWS_SIGNED_DIR="$work/signed" scripts/package.sh "$version"

    # Both zips unpacked and compared file by file.
    for z in unsigned signed; do
        rm -rf "$work/$z-zip"
        if [ "$z" = signed ]; then zip=target/dist/$name.zip; else zip=$work/unsigned.zip; fi
        powershell -NoProfile -Command \
            "Expand-Archive -LiteralPath '$zip' -DestinationPath '$work/$z-zip'"
        (cd "$work/$z-zip" && find . -type f | LC_ALL=C sort) > "$work/$z.list"
    done
    if ! cmp -s "$work/unsigned.list" "$work/signed.list"; then
        echo "signpath.sh: the signed zip holds other files than the unsigned one:" >&2
        diff "$work/unsigned.list" "$work/signed.list" >&2 || true
        exit 1
    fi
    changed=$(while read -r f; do
        cmp -s "$work/unsigned-zip/$f" "$work/signed-zip/$f" || printf '%s\n' "$f"
    done < "$work/signed.list")
    # scripts/zip.ps1 names entries from inside the staging directory.
    wanted=$(printf './bin/%s\n' greycard-ui.exe greycard.exe)
    if [ "$changed" != "$wanted" ]; then
        echo "signpath.sh: signing changed other files than the two executables:" >&2
        printf '%s\n' "$changed" >&2
        exit 1
    fi
    for f in greycard-ui.exe greycard.exe; do
        cmp "$work/signed/$f" "$work/signed-zip/bin/$f"
    done
    echo "the signed zip differs from the unsigned one in its two executables alone"
    ;;
*)
    echo "usage: scripts/signpath.sh request|package [version]" >&2
    exit 1
    ;;
esac
