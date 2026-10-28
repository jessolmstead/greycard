#!/bin/sh
# Make the disk image's window layout, the .DS_Store that
# scripts/package.sh copies into every image beside background.tiff:
#
#     sh packaging/dmg/layout.sh
#
# Run by hand on a Mac, and the two files committed, when the
# background or the layout changes; never at release. Finder writes the
# layout only when told to through AppleScript, which a CI runner
# cannot be relied on to allow, so the release copies the result
# rather than making it.
#
# The .DS_Store finds the background through the volume's name, so the
# volume here and in package.sh must both be "greycard". The icon
# positions match background.swift's arrow; change one and change the
# other.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
volume=greycard
if [ -d "/Volumes/$volume" ]; then
    echo "eject /Volumes/$volume first" >&2
    exit 1
fi

work=$(mktemp -d)
# Only the image this script attached is ever detached, by its device.
device=
trap '[ -n "$device" ] && hdiutil detach -quiet "$device"; rm -rf "$work"' EXIT

swift "$here/background.swift" "$work"
tiffutil -cathidpicheck "$work/background.png" "$work/background@2x.png" \
    -out "$here/background.tiff"

# What the release image holds, by name: an app (an empty bundle is
# enough, since the layout keeps names and positions, not contents),
# the Applications link, and the background in a hidden folder.
root="$work/root"
mkdir -p "$root/greycard.app" "$root/.background"
ln -s /Applications "$root/Applications"
cp "$here/background.tiff" "$root/.background/background.tiff"
hdiutil create -quiet -volname "$volume" -srcfolder "$root" -fs HFS+ \
    -format UDRW "$work/layout.dmg"
device=$(hdiutil attach -noautoopen "$work/layout.dmg" | awk 'NR == 1 { print $1 }')

osascript <<EOF
tell application "Finder"
    tell disk "$volume"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set the bounds of container window to {200, 120, 840, 520}
        set opts to the icon view options of container window
        set arrangement of opts to not arranged
        set icon size of opts to 128
        set text size of opts to 13
        set background picture of opts to file ".background:background.tiff"
        set position of item "greycard.app" of container window to {170, 190}
        set position of item "Applications" of container window to {470, 190}
        update without registering applications
        delay 1
        close
    end tell
end tell
EOF

# Finder writes .DS_Store as the options are set and again when the
# window closes, a moment after: wait until it has stopped changing
# for two seconds, up to half a minute.
store="/Volumes/$volume/.DS_Store"
last= same=0
for _ in $(seq 30); do
    now=$(stat -f %m.%z "$store" 2>/dev/null || true)
    if [ -n "$now" ] && [ "$now" = "$last" ]; then
        same=$((same + 1))
        [ "$same" -ge 2 ] && break
    else
        same=0
    fi
    last=$now
    sleep 1
done
if [ ! -f "$store" ]; then
    echo "Finder wrote no layout; is it allowed to be scripted?" >&2
    exit 1
fi
sync
cp "$store" "$here/DS_Store"
echo "wrote $here/background.tiff and $here/DS_Store"
