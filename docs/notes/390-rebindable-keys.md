# Rebindable keys (2026-10-21)

The roadmap's line: a sheet in Settings to change any shortcut, saved
in settings.json, with a conflict warning and a reset to defaults.
Asked for on issue 31.

**An action.** A line of the keys table with one chord is an action,
named in the table (`act("crop", &["R"], "Crop, and Done")`), 29 in
all. Lines of one action move together: J's two are one, and S is the
proof's, so the grid's stack toggle follows it. Fixed: every line of
several chords (arrows, Home and End, ratings, flags, labels, + and −,
F6 to F8, [ ], Delete, Redo's two, the filter's), Enter, Tab, and
Escape, which cancels a listening row and closes every sheet. A
focused slider's own keys and typed-value editing stay too.

**One place.** The handler in app.slint is unchanged: it compares each
action by its default chord, which is now the action's name there.
`chord()` names the press and looks it up in a thread-local `Keymap`:
a press bound to an action reads as that action's default, a fixed
chord as itself, a vacated default as nothing. So a swap lands once,
a fixed key is shadowed rather than doubled, and the table/handler
test holds as it did. A chord prints Ctrl, Alt, Meta, Shift, then the
key; parses in any order or case, Cmd for Ctrl, and either sheet's
spelling. F1 to F12, Page Up, Page Down, and Insert are named now.

**settings.json.** `"keys": {"crop": "Ctrl+R"}`, only the actions
moved; written at each binding and carried across the close's write.
On read, a name that is no action or a value that is not a chord is
logged and dropped, and a `keys` that is no map reads as empty, so
one bad line never resets the file.

**The sheet.** Settings ends with KEYS: the table's groups, each
action's words and a chord button. A click listens; the next press
with a key binds, a modifier alone waits, Escape keeps the old. A
chord two things share gets a red border and one line, "Also: Undo",
on both rows, and a warning in the log; the first action in the table
takes the press. We warn rather than refuse, since a swap passes
through a conflict on its way. Reset to defaults clears every binding.
The `?` sheet shows the bindings in force.

**Review.** A fresh reviewer found the sheet's focus handling wrong
three ways: closing the sheet or pressing Reset while a row listened
left the focus on a destroyed or unreachable element, every window
key dead; and the key being bound, held, auto-repeated into the
window (Ctrl+O opened the folder chooser). The sheet's close and
Reset now hand the keys back, and repeats are ignored until the first
fresh press. A hand-edited `"crop": 5` reset the whole file; it reads
leniently now. Found on the way: a row's wrapped words counted as one
line, so Reset sat past the scroll's end.

**Not done.** The Keys section has no search. An action bound to a
key a focused slider holds does not fire while the slider has focus,
and the sheet does not say so.
