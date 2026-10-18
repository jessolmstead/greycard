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
files, and the one filter as `NAME | *.a *.b` to zenity and in Qt's
form, `NAME (*.a *.b)`, to kdialog, which splits its filter on `|`.
A save through zenity asks before it replaces a file
(`--confirm-overwrite`, a no-op on zenity 4, which always asks), as
the portal's GTK chooser does. We try both because a bare window
manager tends to have one or the other, zenity far more often; rfd
falls back to zenity alone.

Exit 1 is a cancel, except that GTK also exits 1 when it cannot
open the display, and says "cannot open display" on stderr (Qt says
"could not connect to display" and aborts): that is an error naming
the line. Only those phrases count, since GTK mentions the display
in warnings on a dialog that works, as Xlib's missing RANDR under
Xvfb does on every run. zenity takes its exit codes from `ZENITY_OK` and the like
when they are set, so the child gets none of them. A path picked to
open comes back through the helper's stdout, one a line, and a name
with a newline in it splits in two; kdialog prints in the locale's
encoding, and zenity under `LANG=C` converts too, so a name that is
not UTF-8 or not ASCII can come back changed. The command-line
programs give no way around that, so a returned path that is not
there is said rather than opened.

"Cannot be asked" is narrower than "failed". Only two errors go to a
helper. `Unreached::Bus` is no session bus to connect to.
`Unreached::Portal` is the `OpenFile` or `SaveFile` call failing with
a D-Bus error that means nothing was there to take it:
`ServiceUnknown` or `NameHasNoOwner` (no portal), `UnknownMethod`,
`UnknownInterface` or `UnknownObject` (a portal with no file chooser
backend configured), a `Spawn.*` error or `TimedOut` (a portal the
bus could not start, or not in time), or systemd's `NoSuchUnit`,
`UnitMasked` or `LoadFailed`, where the bus starts the portal as a
unit (masked by hand, as some i3 users do to skip its start-up
wait). Nothing is on the screen until that call returns, so the
user has seen no dialog. Any other error from the call is a portal
that took the request and refused it, as `InvalidArgument` for
§88's empty filter: that is our mistake, and a helper answering
instead would hide it. `NoReply` is not on the list: the call
reached a portal, which may still put its dialog up.

A portal that answered and then failed is reported as it is rather
than asked again in another program: the chooser went away, or a
reply without a file. The `Response` code is the exception. 1 is the
Cancel button, but 2, "ended some other way", is what the GTK and
GNOME backends send when the chooser's window is closed or Escape is
pressed, and also what the portal sends when its backend fails after
the call returned, as xdg-desktop-portal-gtk does when it cannot
reach the display. From here the two look the same, and the first
is the everyday one, so 2 is a cancel with a warning in the log;
the portal's own log says which it was. (A first pass made 2 an
error, which turned every Escape on GNOME into "the file chooser
failed"; the review caught it before it shipped.)

## Saying what is missing

Every chooser's error used to show the same line, which hid the
cause: the empty filter of §88 and a missing bus read the same.
`export::chooser_failed` now writes the line wherever a chooser's
error is shown: no session bus says to install zenity or see the
user guide, no portal says to install xdg-desktop-portal-gtk or
zenity, and anything else is "the file chooser failed:" and the
error with its causes, so a refused call names the D-Bus error. The monitor and soft-proof profile pickers, which only logged
a failure, show it too. The user guide's Before you start has the
Linux line: a portal, or zenity, or a folder named on the command
line.

## Left alone

Report a problem shows the log's folder through the file manager's
`ShowItems` over the same bus, and without one through `xdg-open` on
the folder. On the tester's machine that opened RawTherapee, the
application their desktop had for `inode/directory`. That is the
desktop's choice, and `xdg-mime default` changes it; we leave the
fallback as it is.

## Tested

`route` is tested apart from the bus: the portal's own answer
stands, a portal that failed once asked is not asked again, an
unreached one takes the helper's answer, and with no helper the
unreached error and its line come back. `absent_name` is tested
against the names above and against `InvalidArgs`, `NoReply` and
the portal's own `InvalidArgument`. `Helper::run` takes the program
by path, so shell-script stand-ins check the exits: two paths, a
cancel, a cancel with a display warning, GTK's display failure, a
crash, a path not there, a save's new file, which is not checked,
and a program not installed, which is skipped. The arguments for each
mode are tested against what zenity and kdialog document.
