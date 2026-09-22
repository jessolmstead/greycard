#!/bin/sh
# Install greycard into your home directory. No root, nothing outside
# $HOME:
#
#     sh install.sh
#
# On Linux, the binaries go to ~/.local/bin and the desktop entry and
# the icon under ~/.local/share. libwebgpu_dawn.so is a binary as far
# as this is concerned: it has to sit next to the executables, which
# look for it beside themselves and nowhere else. On macOS the tarball
# holds greycard.app instead, which goes into ~/Applications, and the
# command line is linked out of it into ~/.local/bin so it keeps the
# library beside it. Set PREFIX to install somewhere other than
# ~/.local, and on macOS APPLICATIONS to put the app somewhere other
# than ~/Applications. uninstall.sh takes it all back out.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
bin_dir="$prefix/bin"
data_dir="$prefix/share"

# The tarball says which desktop it was rolled for.
if [ -d "$here/greycard.app" ]; then
    layout=app
elif [ -d "$here/bin" ]; then
    layout=bin
else
    echo "install.sh: run this from the unpacked greycard tarball" >&2
    exit 1
fi

mkdir -p "$bin_dir"

if [ "$layout" = app ]; then
    apps=${APPLICATIONS:-$HOME/Applications}
    mkdir -p "$apps"
    echo "installing into $apps and $bin_dir"

    rm -rf "$apps/greycard.app"
    cp -R "$here/greycard.app" "$apps/greycard.app"
    # A browser marks what it downloads as quarantined, the archive
    # utility passes the mark on to what it unpacks, and Gatekeeper
    # will not open an app that carries it unless Apple has notarized
    # the app, which this one is not. The copy is yours now, from a
    # tarball you chose to unpack, so the mark comes off it.
    xattr -dr com.apple.quarantine "$apps/greycard.app" 2>/dev/null || true
    echo "  $apps/greycard.app"

    # Links rather than copies: each binary finds Dawn beside itself,
    # and the link resolves to where that is.
    for name in greycard greycard-ui; do
        rm -f "$bin_dir/$name"
        ln -s "$apps/greycard.app/Contents/MacOS/$name" "$bin_dir/$name"
        echo "  $bin_dir/$name -> greycard.app"
    done

    echo "done. greycard is in Applications: open it from Launchpad or"
    echo "Spotlight, or with \`open -a greycard\`. greycard is the command"
    echo "line."
else
    mkdir -p "$data_dir"
    echo "installing into $prefix"

    for f in "$here"/bin/*; do
        name=${f##*/}
        # Removing first, because copying over a binary that is running
        # fails with "text file busy".
        rm -f "$bin_dir/$name"
        cp "$f" "$bin_dir/$name"
        case "$name" in
        *.so | *.so.* | *.dylib | *.dll) chmod 0644 "$bin_dir/$name" ;;
        *) chmod 0755 "$bin_dir/$name" ;;
        esac
        echo "  $bin_dir/$name"
    done

    if [ -d "$here/share" ]; then
        cp -R "$here/share/." "$data_dir/"
        find "$here/share" -type f | while read -r f; do
            echo "  $data_dir/${f#"$here"/share/}"
        done

        # The desktop's own caches. None are fatal: the entry, the icon,
        # and the MIME type are already on disk where a session will find
        # them at next login or when the caches are rebuilt. The icon
        # cache only gets rebuilt if this really is a theme directory; a
        # fresh ~/.local/share/icons/hicolor has no index.theme, and the
        # lookup falls through to the system hicolor theme anyway.
        if command -v update-desktop-database >/dev/null 2>&1; then
            update-desktop-database "$data_dir/applications" 2>/dev/null ||
                echo "  (update-desktop-database failed; harmless)"
        fi
        if command -v update-mime-database >/dev/null 2>&1 &&
            [ -d "$data_dir/mime/packages" ]; then
            update-mime-database "$data_dir/mime" 2>/dev/null ||
                echo "  (update-mime-database failed; harmless)"
        fi
        if command -v gtk-update-icon-cache >/dev/null 2>&1 &&
            [ -f "$data_dir/icons/hicolor/index.theme" ]; then
            gtk-update-icon-cache -f -t "$data_dir/icons/hicolor" 2>/dev/null ||
                echo "  (gtk-update-icon-cache failed; harmless)"
        fi
    fi

    echo "done. greycard-ui is the editor, greycard the command line."
fi

case ":${PATH-}:" in
*":$bin_dir:"*) ;;
*)
    echo
    echo "note: $bin_dir is not on your PATH, so \`greycard\` will not be"
    echo "      found by name yet. Add it, for example:"
    if [ "$layout" = app ]; then
        echo "      echo 'export PATH=\"$bin_dir:\$PATH\"' >> ~/.zprofile"
        echo "      then open a new terminal window."
    else
        echo "      echo 'export PATH=\"$bin_dir:\$PATH\"' >> ~/.profile"
        echo "      then log out and back in."
    fi
    ;;
esac
