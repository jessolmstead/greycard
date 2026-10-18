# §346. A file chooser without a portal (2026-10-17)

A tester on Void Linux under i3 had no file chooser at all: Open
folder and Ctrl+O said "the desktop offered no file chooser", and
the log said why, `can't open /run/user/1000/bus`. Their session has
no D-Bus session bus, so `DBUS_SESSION_BUS_ADDRESS` is unset and
nothing listens at `$XDG_RUNTIME_DIR/bus`, the place zbus looks next.
On Linux the chooser was the desktop portal and nothing else (§88),
so with no bus there was no way to pick a folder short of giving one
on the command line. A window manager started bare from `startx` is
an ordinary way to run Linux, and even with a bus it often has no
portal backend for the file chooser.

## zenity, then kdialog

When the portal cannot be asked at all, `export::choose` runs zenity,
and kdialog when zenity is not installed. Each draws a chooser of its
own and prints the paths picked, one a line, exiting 1 for a cancel.
The request goes over as it would to the portal: the folder it opens
on, the name a save offers, a folder chooser or a chooser of several
files, and the one filter as `NAME | *.a *.b` to zenity and
`*.a *.b|NAME` to kdialog. A save through zenity asks before it
replaces a file (`--confirm-overwrite`, a no-op on zenity 4, which
always asks), as the portal's GTK chooser does. We try both because
a bare window manager tends to have one or the other, zenity far
more often; rfd falls back the same way.

"Cannot be asked" is narrower than "failed". The portal's errors up
to and including the `OpenFile` or `SaveFile` call carry
`Unreached::Bus` (no session bus) or `Unreached::Portal` (a bus, but
no portal or no file chooser behind it), and only those go to a
helper. Nothing is on the screen until that call returns, so the
user has seen no dialog. A portal that answered and then failed (the
chooser went away, a reply without a file) is reported as it is
rather than asked again in another program.

## Saying what is missing

Every chooser's error used to show the same line, which hid the
cause: the empty filter of §88 and a missing bus read the same.
`export::chooser_failed` now writes the line wherever a chooser's
error is shown: no session bus says to install zenity or see the
user guide, no portal says to install xdg-desktop-portal-gtk or
zenity, and anything else is "the file chooser failed:" and the
error. The user
guide's Before you start has the Linux line: a portal, or zenity,
or a folder named on the command line.

## Left alone

Report a problem shows the log's folder through the file manager's
`ShowItems` over the same bus, and without one through `xdg-open` on
the folder. On the tester's machine that opened RawTherapee, the
application their desktop had for `inode/directory`. That is the
desktop's choice, and `xdg-mime default` changes it; we leave the
fallback as it is.
