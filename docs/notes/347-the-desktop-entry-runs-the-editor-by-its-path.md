# §347. The desktop entry runs the editor by its path (2026-10-17)

The desktop entry said `Exec=greycard-ui %f`, so a menu, a file
manager's Open with and a raw's double-click all found the editor on
the session's PATH. install.sh puts it in `~/.local/bin`, which
Debian and Ubuntu put on PATH from `~/.profile` and Void, Arch and a
window manager started from `startx` often do not. install.sh warned
about that at the end, but the warning is about typing the name, and
a tester who never sees it gets a menu entry that does nothing.

install.sh now writes the full path into the installed entry,
quoted: `Exec="/home/you/.local/bin/greycard-ui" %f`. The Desktop
Entry spec wants a path with a space in it quoted, and inside the
quotes `"`, `` ` ``, `$` and `\` need escaping twice over (once for
the quoting, once for the file's own string escapes). A `PREFIX`
with any of those keeps the bare name rather than risk a line the
desktop reads wrong; that is the old behavior, not a broken one.
The entry in `assets/` keeps the bare name, since the tarball does
not know where it will go, and uninstall.sh removes the file as
before. Typing `greycard-ui` in a terminal still needs PATH, which
the warning still covers.

Checked by installing into a plain prefix, one with a space, `&` and
`|` in it, and one with `$`: the first two get the quoted path and
pass `desktop-file-validate`, the third keeps the name.
