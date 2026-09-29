# 171. A menu on a frame, and copy and paste of settings (2026-09-25)

Two roadmap lines that share a surface: "Right click on photo for
options. Set label, copy develop settings, etc", and the backlog's
"Settings copied and pasted from a right-click: the sync's loop with a
clipboard between (§156, §162). There is no context menu on the strip
or the grid yet." Wave C, an opus author and an opus reviewer, on
d9b224e.

**The menu, and which frames it is about.**

A right-click over a frame in the strip, the grid or the viewport opens
one menu: Copy settings of FILE; Paste settings (grayed when nothing is
copied, and naming the frame copied when something is); a Rating
submenu (No stars to 5 stars); Pick, Reject, Unflag; a Label submenu
(None and the five colors); Reveal in file manager; Export...

Which frames it acts on is `selection::right_click(set, current,
file)`, a pure function beside `selection::click`, returning the same
`Click`. On a frame in the selection the selection stays as it is and
the menu is about all of it. On a frame outside it the frame is opened
first, as a plain click opens it, and the menu is about that frame
alone. That is the file manager's rule, and the reason for it is the
same: a menu opened on one thing must not act on other things chosen
earlier somewhere else on screen. The modifiers do not count; a
right-click is not a way to grow the set. The viewport's right-click
is about the current frame, which is always in the set, so it acts on
the whole selection.

The checks are filled when the menu opens (`panel::menu::menu_asked`):
the rating, flag and label the whole selection shares, none when it
differs. The label items keep the label key's toggle (a label every
frame already wears is taken off, `meta::Change::settled`), and the
check is what makes that readable: a checked Red unchecks. The Export
item says "Export 3 frames..." over a set of three, since §167's sheet
exports the selection. With nothing chosen (the loupe under a filter
that hides every frame) the rating, flag and label items are grayed.

**Every item is a key's code.** Rating, flag and label go through
`browser::meta_on_selection`, the body of the culling keys' handler
pulled out into a function the key and the menu both call; Purple and
"no label" are reachable from the menu and from no key.
`panel::menu::menu_change(kind, value)` turns an item into the
`meta::Change` its key makes, and a test holds it against
`Change::from_key` for every key. Paste is the Ctrl+V handler. Export
goes through `export-asked()`, a public function the Ctrl+Shift+E key
now calls too, so the rule for when the sheet may open is written
once.

Two items differ from their keys, on purpose:

- **Copy** takes the frame right-clicked, and the item names it
  ("Copy settings of 4Z4A3521.CR3"); Ctrl+C takes the frame on screen.
  The first cut copied the frame on screen from the menu too, so a
  right-click on another frame of the set, then Copy, copied a frame
  the pointer was not on, while Reveal beside it followed the pointer.
  `copy_settings_of(file)` takes the panel's live edit when the file
  is the frame the panel holds, and otherwise the sidecar's current
  state after `migrate_frame`.
- **Export...** is not grayed while a develop runs. A right-click on a
  frame outside the selection opens that frame and starts its develop,
  and the first cut gated the item on `busy` as Ctrl+Shift+E is gated;
  the review found the item stayed grayed with the menu still open
  after the develop had landed, since the menu is not rebuilt while it
  is up. The item now asks only that a frame be open, no export be
  running, and culling be off. Chosen during a develop, it sets
  `export-waiting`, and the sheet opens when `busy` falls
  (`changed busy` on the window). Ctrl+Shift+E still refuses while
  busy, as before.

Reveal is the only new action. It is one command per platform, chosen
by `cfg` when the editor is compiled: `open -R FILE` on macOS, which
selects the file in the Finder; `explorer /select,"FILE"` on Windows,
passed as a raw argument because Explorer does not accept the quoting
Rust would put around the whole argument (the argument is built by
`explorer_select`, a pure function tested on every platform);
`xdg-open FOLDER` elsewhere, which opens the folder but cannot select a
file in it. The path is made absolute first. The review launched the
editor as `greycard-ui 5M0A3021.CR3` from inside the folder, and the
first cut ran `xdg-open ""`: a relative name's parent is empty. The
child is reaped on a thread of its own so it is never left a zombie,
and a failed start or a non-zero exit is logged. The `open` crate is
not a dependency, and three `Command`s did not justify adding it. The
Freedesktop `FileManager1.ShowItems` D-Bus call would select the file
on Linux; it was left out because not every file manager implements
it.

**Slint's ContextMenuArea, and where it lives.** The menu is Slint
1.18's `ContextMenuArea` with `Menu` and `MenuItem`
(`ui/panel/frame-menu.slint`). On macOS and Windows the winit backend
shows it as the platform's own menu (muda); on Linux Slint draws it as
a popup in the window's style (fluent-dark here). Under the editor's
setup (winit on Xwayland, femtovg on wgpu) it drew correctly, placed
where asked and kept inside the window at the strip's edge, and the
snapshots show it. No fallback popup was needed.

There is one menu for the window, not one per cell. A
`ContextMenuArea` opens on the right-button press it receives, before
any code of the editor's can run, so it cannot make the clicked frame
current first; and hundreds of grid cells each holding a menu tree
would be wasteful. Instead the `FrameMenu` is a zero-sized area at the
window's origin with `enabled: false`, so it never takes a click of
its own, and it is opened by its `show` function. Each strip and grid
cell's TouchArea, and the viewport's, sees the right-button press in
its `pointer-event`, reports the row and the point in window
coordinates (`absolute-position` plus the mouse), and the window's
`frame-menu-at` asks Rust to settle the target and then opens the
menu there. The menu does not open under a sheet. In the viewport it
does not open with a tool in hand (a shape being placed, a repair, a
dropper, the level, a guide), since placing and the retouch use the
right button for themselves.

**The press that closes the menu.** A press outside Slint's menu closes
it and is still delivered to what is under the pointer, which is Slint
1.18's behavior (`WindowInner::process_mouse_input` closes the popup
after the dispatch). In the first cut, a left click to dismiss the
menu over the picture zoomed it to 100%, and over a cell opened that
frame. The fix is for the surfaces to let that one press go by, which
needs them to know the menu is up. `ContextMenuArea` in Slint 1.18
does not say so: its `is-open` belongs to the internal element and is
not reachable from `.slint`, and there is no closed callback. So the
window reads it off the keys. Slint's menu takes the focus when it
opens and gives it back when it closes, whether by a choice, a click
outside or Escape. The window's key FocusScope sets `menu-up` on
`focus-lost` with reason `popup-activation` and clears it on any
`focus-gained`. A strip or grid cell pressed with `menu-up` set takes
no click from that press; the viewport takes no zoom and no pan. Two
things could give the keys back early, and both are guarded: the
viewport's `focus-keys` on hover and on press does nothing while the
menu is up. A native menu (macOS, Windows) takes neither the focus nor
the click, so `menu-up` never sets there and nothing is swallowed.

On a Mac, Control+click is a right-click. Slint maps the Mac's Command
key to its `control` modifier and the Control key to `meta` (the winit
backend swaps them on Apple platforms), so the cells treat a left press
with `meta` held as a right-click when the window's `ctrl-click-menu`
is set, which `panel::menu::install` sets from `cfg!(target_os =
"macos")`. The same swap means Ctrl+C and Ctrl+V as bound here fire on
Command+C and Command+V on a Mac with no code for it, as every Ctrl
binding in the window already did; the user guide's line saying the
Mac's keys were Ctrl "for now" was wrong and is corrected.

**Copy and paste.**

Copy puts an edit, whole, into `State::clipboard`: from the menu, the
frame right-clicked (above); from Ctrl+C, the frame on screen, the
panel's edit outside culling, recorded or not, since what is on screen
is what is meant, and the sidecar's current state in culling, where
the panel is not the frame's. It keeps the source's path, not its
index, because moving the rejects or opening another folder renumbers
the files and the clipboard outlives both. It is not the system
clipboard. An edit is not text, nothing else on the desktop reads it,
and a paste needs the source's path, which the system clipboard would
not carry. It lasts for the session. `crate::clipboard` holds the pure
parts: what a copy holds, its name and label, the targets (the set
less the source, by path), the `Preset::from_edit` of the chosen
sections, and the learned-denoiser source when Noise is chosen.

Paste opens the sync sheet itself, `SyncSheet` with a `paste` flag:
titled "Paste settings from FILE.CR3", the button reading Paste, the
line saying "Onto 2 frames; 3G0A4650.CR3, which they came from, is
left as it is." when the source is in the selection. The sections are
checked as the last sync or paste this session left them
(`State::sync_last`, set on either's apply), or the sync's defaults
before either has run. The sync's own sheet still opens on its
defaults, as §156 left it, so that line of §156's Left is unchanged.
Closing the sheet any way clears the paste flag (`changed sync-open`),
so a canceled paste leaves nothing that could make the next opening a
paste.

The apply is `panel::sync::paste_selection`. It is a sync with the
clipboard between: the targets other than the frame on screen go
through `lay_over_targets` exactly as a sync's targets do (migrate,
the camera profile's fit per body, the learned blend seeded or
carried, one step each, the sidecar written where the setting puts it,
thumbnails and badges refreshed). Outside culling, the frame on screen
is the panel's. It takes the paste the way a preset click takes a
preset: the panel's state recorded first, the paste over it as one
step, the same `preset_for_body` fit check, and `take_current` to
develop it, with the status words riding the develop through
`status_after_develop`. In culling every frame, the one on screen
included, goes through the sidecars, and culling is not left. A preset
click in culling does leave it, but a paste there is a culling action,
so it stays. Geometry and retouch are left out because they are not
`Section`s, as in a sync. Noise brings the learned tier and blend, as
a sync's does, because the paste's source is one frame of the user's
own.

Each frame's step is recorded as "Paste from FILE.CR3"
(`history::paste_label`, beside `sync_label`; it fits the history
row's 36 characters with a camera's file name). A paste onto the
source frame alone opens nothing and says "FILE is the frame these
came from; choose the frames to paste onto". A paste the frames
already have records nothing ("the 2 frames had these already"). As
with the sync sheet, opening the paste sheet records `(current,
targets)` in `State::paste_asked`, and an Apply after the selection
moved is refused: "the selection changed while the sheet was open;
nothing pasted".

**Keys.** Ctrl+C and Ctrl+V are bound in the window's FocusScope, the
place where the keys are nobody's. A focused text field (the filter's,
which is also where keywords are searched) handles its own Ctrl+C and
Ctrl+V first and accepts them, so they never reach the binding; a test
types into the grid's filter field and presses both, and the clipboard
stays empty. Under a sheet both do nothing, as undo does. Neither
clashes with the bare C (culling) or V (compare), which check that
Ctrl is not held.

**Snapshot flags.** `--menu ROW` opens the menu over that row of the
strip, or of the grid with `--grid`, as if right-clicked in the cell's
middle. It works through a `menu-at` property that the strip and the
grid watch, since the grid is a conditional element whose cells the
window cannot reach. `--sheet paste` copies the first frame and opens
the paste sheet over the `--also` set. `--sheet pasted` applies it on
the sheet's sections in the same turn. A `--menu` over a frame outside
the set starts a develop that can land after the capture; a snapshot
run no longer writes `last_file` to the settings when it does
(`remember_last_file` checks `State::batch`, which outlives the
snapshot's path).

**Measured.** Copies of four sample raws (3G0A4650.CR3, 4Z4A2978.CR3,
4Z4A3521.CR3, 5M0A3021.CR3) in target/work, the editor on its own
headless mutter's Xwayland with every XDG directory under target/work.

- `--grid --menu 2`: the menu drawn over the third cell, which became
  the current frame (the header reads 4Z4A3521.CR3), the first item
  "Copy settings of 4Z4A3521.CR3", Export... enabled while that frame
  developed; `settings.json` byte-identical before and after
  (`menu-grid-2.png`).
- `--grid --also 1,2 --menu 2`: right-click inside a set of three. The
  set stayed and the header kept "3G0A4650.CR3 and 2 others". The item
  read "Export 3 frames..." (`menu-grid-set.png`).
- `--menu 1`: over the strip, the menu kept inside the window above
  the strip's bottom edge (`menu-strip.png`).
- `--also 1,2 --sheet paste`: the sheet titled "Paste settings from
  3G0A4650.CR3", "Onto 2 frames; 3G0A4650.CR3, which they came from,
  is left as it is.", 17 of 18 sections on (`paste-sheet.png`).
- `--preset "Warm Negative" --also 1,2,3 --sheet pasted`: the paste
  onto three frames, three sidecar writes included, took about a
  millisecond (0.6 ms in this run, 1.1 ms in the reviewer's). Each of
  the three new 8838-byte sidecars has `step: "Paste from
  3G0A4650.CR3"` and one earlier state (the default); the source's own
  sidecar has only its "Preset: Warm Negative". With one target
  (`--also 1`), `jq -S .current` of the source and the target were
  identical. (That the learned blend travels with Noise is held by the
  unit test's 0.6 blend, not by these files: every sample here is ISO
  200 or less, where `blend_for_iso` gives 0.35 anyway.)
- The pasted frame reopened (`4Z4A2978.CR3 --snapshot`): its history
  panel reads "Paste from 3G0A4650.CR3" above "Original"
  (`history-paste.png`).
- The closing press, on the real winit backend: a temporary hook in a
  `--grid --menu 2` run dispatched events through the window 800 ms
  after the menu opened (the same `dispatch_event` path the platform's
  events take once winit has them). It logged `menu_up=true`, then
  after an Escape `menu_up=false`; and in a second run, a left click
  on cell 0 with the menu up left the selection on row 2 and cleared
  `menu_up`, and the next click on cell 0 opened it. The hook was
  removed.

Input can be driven headless through mutter's
`org.gnome.Mutter.RemoteDesktop` and `ScreenCast` D-Bus API; the
reviewer's driver is at `target/review/rd.py`. In the author's own run
of it the pointer did not move (the stream's absolute motion had no
effect, maybe because nothing consumed the PipeWire stream), so the
real right button was exercised on Slint's testing backend (pointer
events dispatched at strip cells and the picture) and by the reviewer
through the driver.

**Tests added.**

- `selection.rs`:
  `a_right_click_in_the_set_keeps_it_and_one_outside_opens_the_frame`.
- `clipboard.rs`: `a_copy_holds_the_whole_edit_and_the_frame_it_came_from`,
  `a_paste_leaves_out_the_frame_it_was_copied_from` (by path, surviving
  a renumbered folder), `a_paste_lays_the_chosen_sections_and_no_geometry`
  (Light alone; every section without the geometry; Noise carrying
  the learned tier and a 0.6 blend; the frame on screen's edit equal to
  what a target's sidecar records; a second paste records nothing).
- `panel/menu.rs`: `a_menu_item_is_the_change_its_key_makes`,
  `the_checks_are_what_the_whole_selection_shares`, the Reveal command
  per platform (one `cfg`'d test each for Linux, macOS and Windows),
  `explorers_argument_is_one_quoted_select` (every platform),
  `reveal_of_a_relative_name_opens_the_working_folder`,
  `a_right_click_outside_the_set_opens_the_frame_and_inside_keeps_the_set`
  (real right-button pointer events on the strip; a menu rating
  reaches the whole set as the key does; flag and Purple from the
  menu), `a_macs_control_click_is_a_right_click_and_not_a_click`,
  `the_menus_copy_takes_the_frame_right_clicked_and_ctrl_c_the_one_on_screen`,
  `the_press_that_closes_the_menu_is_not_a_click` (over the strip, by
  a click and by Escape), `the_press_that_closes_the_menu_over_the_picture_does_not_zoom`,
  `the_menus_export_waits_for_the_develop`, and
  `the_viewport_menu_is_the_frame_on_screen_and_its_set`.
- `panel/sync.rs`: `copy_and_paste_lay_the_copy_over_the_set_and_leave_its_source`
  (Ctrl+C and Ctrl+V as keys; nothing copied; the source alone; the
  sheet's words and checks; Light alone applied, labels in memory and
  on disk, a crop kept, the source and a frame outside the set
  untouched; the next paste opens on that choice and records nothing;
  a canceled paste clears the flag; the sync's own sheet still on its
  defaults),
  `a_paste_onto_the_frame_on_screen_goes_through_the_panel_and_undoes`,
  `a_paste_in_culling_goes_onto_the_sidecars_and_stays_in_culling`,
  `a_selection_moved_under_the_paste_sheet_is_refused`,
  `the_filter_field_keeps_its_own_copy_and_paste`.
- `greycard-edit`: `paste_label` held to the history row's width with
  the other labels.
- `Shown::sheet` accepts `paste` and `pasted`.

The existing sync tests are unchanged and pass.

**The guide.** A paragraph on the menu and on copy and paste, and a
Ctrl+C, Ctrl+V row in the keys. Two older lines were corrected on the
way: "Export works on the open frame only" was stale since §167 and
now says what a set's export does, and the Mac line now says the Ctrl
keys are ⌘ and that Control+click is a right-click.

**Left.**

- Reveal on Linux opens the folder and does not select the file (see
  above).
- The menu shows no key hints: Slint 1.18 allows `shortcut` on a
  MenuItem only in a MenuBar.
- `menu-up` rests on Slint's menu taking and returning the keys'
  focus. Anything that focuses the keys while the menu is up, other
  than the two guarded paths, would clear it early and let the closing
  press through again. Nothing else does now.
- A Paste special that picks sections without the sheet (Lightroom's
  "Paste settings from previous"), and remembering the sync sheet's
  own choice (still §156's Left).
- Not run on a Mac or on Windows: the native menus there (muda), the
  Control+click path and the Reveal commands are compiled by `cfg` and
  covered by the platform's own test in CI, not seen.

**The review.** Land after fixes, nothing blocking, and every finding
reproduced live: Copy from the menu took the frame on screen rather
than the one right-clicked while Reveal beside it followed the
pointer; Export stayed grayed in an open menu after the develop had
landed, so from the menu it never worked on a frame outside the set;
Reveal from an editor launched inside the folder ran `xdg-open ""`;
the click that dismissed the menu zoomed the picture; and two lines
of the user guide were stale, the one saying export works on the open
frame only and the one saying the Mac's keys were Ctrl. One round
fixed all six and the nits (a snapshot run writing `last_file`, the
`explorer` argument untested, the items live over an empty
selection, the paste flag left set by a canceled sheet). The reviewer
also found that input can be driven into the headless mutter through
its RemoteDesktop D-Bus API, which future reviews can use for real
clicks.
