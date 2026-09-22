#!/bin/sh
# Take back out what install.sh put in:
#
#     sh uninstall.sh
#
# Only the files listed below are touched, and only under ~/.local (or
# PREFIX, if install.sh was given one) and, on macOS, ~/Applications
# (or APPLICATIONS). Nothing else in your home directory is read or
# removed; your photographs and sidecars are not ours to delete, and
# neither are your settings or the models you fetched.
set -eu

prefix=${PREFIX:-$HOME/.local}

# This list is the tarball's own layout written out by hand: whatever
# scripts/package.sh puts in and install.sh copies, this removes. Add
# something there and it belongs here too.
files="
$prefix/bin/greycard-ui
$prefix/bin/greycard
$prefix/bin/libwebgpu_dawn.so
$prefix/share/applications/greycard.desktop
$prefix/share/icons/hicolor/scalable/apps/greycard.svg
"
if [ "$(uname -s)" = Darwin ]; then
    files="$files
${APPLICATIONS:-$HOME/Applications}/greycard.app
"
fi

removed=0
for f in $files; do
    # -e follows a link, and a link to an app already gone is still
    # ours to remove, so -L is asked too.
    if [ -e "$f" ] || [ -L "$f" ]; then
        rm -rf "$f"
        echo "  removed $f"
        removed=$((removed + 1))
    else
        echo "  not there $f"
    fi
done

if [ "$removed" -eq 0 ]; then
    echo "nothing to remove"
    exit 0
fi

if command -v update-desktop-database >/dev/null 2>&1 &&
    [ -d "$prefix/share/applications" ]; then
    update-desktop-database "$prefix/share/applications" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1 &&
    [ -f "$prefix/share/icons/hicolor/index.theme" ]; then
    gtk-update-icon-cache -f -t "$prefix/share/icons/hicolor" 2>/dev/null || true
fi

echo "done."
