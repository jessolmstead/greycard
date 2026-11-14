# §664. A sheet holds the focus (2026-11-13)

**The leak.** §658 found Tab walking out of Settings onto the panel,
where the arrows then moved a slider and a history step followed. The
cause was not Settings' own. Slint's Tab walks the whole window's
item tree and has no notion of a modal subtree: a key nothing accepts
moves the focus to the next item anywhere in the window. Every sheet
is a sibling of the panels under the window root, so Tab from any
sheet's last control went on, round the tree's end, into the keys,
the panes, and the develop panel. The window's key handler already
did nothing under a sheet; the leak was the panel's own controls,
whose arrows, Space, and Return act on the edit directly.

Every sheet leaked but the paste/sync sheet, which held its two ends
by hand (§589): model, rejects, delete, look remove, look rename,
time shift, overwrite, backup, archive rejects, match, preset, preset
report, Lightroom, RawTherapee, collection, preset group, root,
export, import, Settings, and keys. Six take no focus as they open
(import, Lightroom, RawTherapee, match, model, and backup) and are
served by the window's keys; there Tab went from the keys straight
onto the panel, and a panel control that had the focus as one opened
(a button clicked, a slider) kept it. Context menus and combo boxes
are Slint popups, whose Tab walk stays inside them: no leak.

**The ring.** Two zero-sized FocusScopes bracket the sheets in
`app.slint`, enabled only while one is up and never focused by a
click. Tab past a sheet's last control lands on the stop after the
sheets; it gives the focus to the stop before them and sends the
window a Tab, so Slint's own walk, which skips what is hidden or
disabled, finds the first control. Shift+Tab is the mirror. Slint has
no focus-next a component can call, so the Tab is sent from Rust
(`focus-step`, `keys::install`), synchronously, inside the stop's
focus-gained. The window's keys, holding the focus under the six,
send Tab and Shift+Tab into the ring the same way, before the key
table sees them, whatever Tab is bound to. Those six give the keys
the focus as they open. The sync sheet's hand-held ends are gone, its
tests passing on the ring.

**No control.** A sheet whose every control is off (the Lightroom
sheet while its import runs) leaves the walk nothing: it goes round
and ends on a stop, which has no keys, so Escape and Return were
dead until the next Tab. After the walk the stop looks where the
focus ended, and on a stop gives it to the window's keys. It looks
rather than waits to be called, since Slint does not run a handler
from inside itself: the stop the walk reaches a second time never
hears it. The Lightroom and import sheets' fields turned Escape away
to nowhere, the window's keys not being their parents; each card now
takes Escape as the window does.

**Closing.** A sheet closed, the last or one of two, hands the keys
to the window, whose Escape serves every sheet and from which Tab
goes into the ring. The preset and root sheets had no hand-back, and
a sheet closed from Rust over another (the model sheet over Settings)
took the focused control with it and left the sheet beneath deaf to
Escape.

**Shortcuts under a sheet.** The key handler was already shut under
a sheet but for three: any key turned the Original off, Escape put
the wheel in hand down, and Ctrl+O opened a folder behind the sheet.
All three now wait for the sheet to close.

**Rejected.** Making every control behind refuse the focus while a
sheet is up: the controls are shared with the sheets, so each use
would have to say which side it is on, and a forgotten one fails
silently. One FocusScope wrapping all the sheets, to catch Tab in its
capture phase: it cannot know a control is the last, and wrapping
would move a thousand lines. A disabled head stop and a programmatic
`focus()` to walk forward from it: Slint's programmatic walk ignores
visibility, so it could land on a hidden control. Deferring the step
to a change handler: two Tabs in one event batch would leak between.

**Upstream.** Slint wants a focus scope (or a `PopupWindow` in-window)
that holds Tab inside a subtree, or a `focus-next()`/`focus-previous()`
a component can call. Until then the stops' re-entrant focus move
leaves Slint tracking the stop rather than the control for that one
step: a control later hidden keeps the focus until the next key or
click, and an IME's candidate window may sit wrong for a field reached
by a wrap.

**Open.** A sheet's close gives the keys to the window, not to the
control that had them before it opened: Slint cannot name the focused
item to restore it. An IME candidate window may be misplaced for a
field reached by a wrap (above). Shift+Tab's delivery (Backtab or
Shift with Tab) is tested headless only, not on Windows, macOS, or X11.

Tests: `keys::the_settings_sheet_holds_the_focus` (§658's case, both
ways); `keys::every_sheet_holds_the_focus` (all 22, each opened over a
focused slider, both ways, Escape closing it at every step, a slider
letting go of the keys at the first);
`keys::a_sheet_closed_over_another_leaves_the_keys_to_the_window`;
`keys::a_sheet_with_no_control_leaves_the_keys_to_the_window`; and
`prefs::a_sheets_field_takes_every_key`.

Found in review and left: text typed into a sheet's field after a
sheet opened over it has closed goes nowhere until the field is
clicked (nothing behind the sheet hears it); with the model sheet over
another, Escape closes the one beneath first, as before; nothing ties
`sheets-up`'s list to `sheet-open()`'s; and the every-sheet test is
the suite's slowest, at about 45 s.
