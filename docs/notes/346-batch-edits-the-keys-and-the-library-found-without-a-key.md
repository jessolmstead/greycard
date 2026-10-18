# §346. Batch edits, the keys and the library, found without a key (2026-10-17)

Three of 0.8.0's polish lines, from two testers with Lightroom habits:
one could not find a batch edit or the shortcuts, and both took a while
to find the library. Each was there behind a key (Ctrl+Shift+S, the
guide's Keys chapter, G); none was where a hand from Lightroom looks.

## A Sync button, and a relative sync

**The button.** With more than one frame selected, the develop panel's
head shows **Sync 4 frames to this one...** under the caption, above
the scopes: the sheet Ctrl+Shift+S opens, over the same selection
(`sync-open-asked`, so the tool-in-hand refusal and the moved-selection
refusal are the ones the key has). It says how many and to which,
because the PRESETS section's **Sync...** said neither and the tester
did not take it for a batch edit. The frame menu's items now say what
they do to the selection in the same words: **Paste settings from X
onto 3 frames...** (the count is the clipboard's targets over the
selection, `menu_paste_count`) and **Sync 3 frames to X...**. Copy
settings of X is unchanged: it acts on the frame right-clicked, not the
selection. PRESETS keeps its **Sync...**.

**Set or shift.** The sync sheet gains a row of three choices,
Temperature, Tint and Exposure, each **Set** (today's sync) or
**Shift**. A shift moves each target from its own value by the
source's difference from where a frame starts: Temperature and Tint
from the source's as-shot light, Exposure from the default edit's
0 EV. A batch shot at 4200 K and 5000 K, synced from a frame warmed
from its as shot, each goes as much warmer from where it was, as
Lightroom's Quick Develop relative arrows move it, and not both to one
value. We read "the source's move" as from its own start because that
is the only start every frame shares the meaning of; a delta against
the targets' values (source minus target) would land every frame on
the source's value, which is Set.

**Temperature shifts in mired**, not kelvin. +300 K is about 33 mired
at 3000 K and 8 at 6000 K, so a kelvin shift would move a warm frame
four times as far as a cool one by eye; Lightroom's raw Temp scale is
close to uniform in mired. The source's difference is
`1e6/as_shot − 1e6/source`, and each target goes to
`1e6/(1e6/own − Δmired)`: warmed 300 K from 4800 K, a frame at 3000 K
moves 115 K and one at 6500 K 560 K. The sheet says the step in mired
and the kelvin it comes to at the source's own light and at 3000 and
6500 K ("Temperature +12 mired (+300 K at this frame's 4800 K, +115 K
at 3000 K, +560 K at 6500 K)"); the status line after the sync says
the mired.

- `Shift` (in `panel::sync`) is the three differences, `None` where
  the field sets, its section does not go (the White balance section
  for the two, Light whole or its Exposure part for the third), or the
  source's start could not be read. `shift_of` makes it; `shifted`
  lays it on one target's copy of the preset, read from the target's
  line before the lay, so each frame still takes one history step,
  "Sync from" the source, and the lay is `lay_over_targets`'s own
  (`lay_shifted_over_targets` is the same loop with the per-frame
  preset).
- **As shot on a target** with a Temperature or Tint shift becomes
  custom at its own as-shot kelvin and Duv, moved. That needs the
  target's as-shot light without opening it: `decode::white_path`
  reads rawler's dummy image (the as-shot gains and the color
  matrices, no samples, as `stance_path` reads the crop), and
  `color::temp_tint_of_gains` resolves them through the profile the
  develop would use (the edit's named DCP after the sync, or the
  raw's own calibrations, `profile_from_calibrations`). A test holds
  it to the decoded frame's `as_shot_temp_tint`, exactly.
- **The reads go on the pool**, before the lay: a read is 9 to 30 ms a
  raw and about 150 ms an Adobe DNG, which one by one on the window's
  thread froze it for a 300-frame sync. `read_whites` reads every
  target at As shot whose white a shift moves in one `par_iter`, after
  `rows::load_frames` has brought the sidecars in the same way, and the
  lay takes the answers by cell. A custom target needs no read, and
  none is made when no white field shifts. The source's own light is
  one read, on the sheet's opening and again at the sync.
- **A source at As shot has moved nowhere**: with either white field on
  Shift, every target's white balance is its own, custom or As shot,
  whatever the other field's choice, and no raw is read for it. A Set
  beside a Shift there would mean "each frame's own as-shot value",
  which leaves the frame as it is too. (The first cut took the
  source's as-shot number for the set field, turning every target
  custom at the source's Duv; a review caught it.) Both on Set is
  today's sync, the source's As shot laid on every frame.
- A target whose raw will not say keeps its white balance; a source
  that is no raw, or whose raw will not say, has no Temperature or
  Tint shift, and the sheet says they are set instead. The results are
  held to the sliders' ranges (2000 to 12000 K, ±0.05 Duv, ±5 EV).
- **The 0 EV baseline is the default edit's.** A frame whose default on
  import set an exposure (a preset applied at import, or the per-body
  correction §342 measured, were it ever written into the edit) starts
  away from 0 EV, and its shift then carries that start along: the
  baseline is wrong for it. Exposure's own start is not kept anywhere
  a sync could read it; the draft leaves it there.
- The choices are kept in `settings.sync_shift` with the panel's other
  choices, at the close; all off by default, the sync every earlier
  build ran. They are greyed while their section is unchecked, and the
  sheet's words say what each frame moves by, which the status line
  repeats after the sync.
- A paste's sheet has no row: a paste sets. A shift there would want
  the clipboard source's as-shot light and the panel frame's path of
  `paste_onto` taught the per-frame preset; left for when a tester
  asks.

Geometry and masks are untouched: the shift changes only the
preset's `white_balance` and `light.exposure` for that frame.

Tests: `a_shift_moves_each_frame_from_its_own_value_and_a_set_does_not`
(the differences, set against shift, the As shot target, the unread
raw, the sliders' ends); `a_temperature_shift_is_the_same_mired_at_every_light`;
`a_source_at_as_shot_leaves_every_frames_white_whatever_is_set` (the
review's case, every pairing of Set and Shift); `a_relative_sync_moves_each_frames_rendered_card_by_the_shift`,
which writes four linear DNGs of a grey card under two lights, syncs
through the window's sheet and renders each target's card through the
develop and the export's finish: each target lands at its own light
moved by the source's mired, custom at the as-shot light renders as
As shot did (within 40 of 65535), every target renders warmer and
brighter than it was, and the two custom frames' cards stay apart
where a set renders them identical; the pool's read is held to the
window thread's;
`the_shift_choices_grey_out_with_their_section_and_hide_for_a_paste`;
`the_panels_sync_button_opens_the_sheet_on_the_selection` (clicked,
Ctrl+clicked set, the sheet's words, the menu's paste count); core's
`the_white_without_the_samples_is_the_frames` and
`the_gains_alone_give_the_frames_as_shot_light`.

## The keys sheet and a Help menu

There was no key table: the window's keys were a 300-line chain in
`app.slint`'s `keys := FocusScope`, each clause comparing the event's
text and modifiers by hand. A sheet copied from it would have gone
stale the first time a key moved, so the table came first.

- `keys.rs` holds `TABLE`: groups in the guide chapter's order (Moving
  around, Views, Rating and sorting, Editing, Files, sheets and help),
  each line its chords, how the sheet writes them, and what they do.
- The handler now asks `key-chord(text, ctrl, shift, alt, meta)` for
  the press's name ("G", "Ctrl+Shift+S", "Shift+Left", "?") and gets it
  back only when the table binds it; an empty name is turned away
  first. Every clause compares that name, never `e.text`. What a chord
  does in which view (S the proof in the loupe and a stack in the
  grid, Escape's long chain of sheets) stays in the handler beside the
  properties it reads; the table is what the window binds and what the
  user is told.
- The names are exact about modifiers where the old clauses were
  loose. A letter is named upper case with its Shift, so Caps Lock is
  no Shift; any other character carries its Shift (a "?" is Shift and
  /, a digit on AZERTY is Shift and the key) and does not say it
  again. Escape is "Esc" whatever is held. The culling keys still hand
  `meta-key` the key's text. Kept, bound in the table as master took
  them, so nothing a Lightroom hand relies on goes silent: Shift+P,
  Shift+X and Shift+U flag; Ctrl+= and Ctrl++ and Ctrl+- and Ctrl+_
  size the grid's tiles (the lightbox's plus and minus, which master
  took only bare, stay bare); Ctrl+Home, Ctrl+End and with Shift go to
  the ends; Esc with any modifier closes a sheet and puts a tool down.
  **Gone on purpose**, since nothing names them and each did what the
  bare key does only because master's clause did not look: Shift, Alt
  or Super with the letter keys master let through (G, C, V, Z, J and
  S took any of them but Ctrl, L and B took Shift), Shift with Space,
  Ctrl or Alt with the arrows, Ctrl+Alt+Z and Ctrl+Alt+Y, and the Ctrl
  keys that ignored a second modifier (Ctrl+Shift+F, Ctrl+Shift+O,
  Ctrl+Alt+Shift+S, Ctrl+Alt+, and their Super forms). Shift+Tab was
  never bound.
- **Never stale**: `every_key_the_handler_binds_is_on_the_sheet_and_every_line_is_bound`
  reads the handler's own source, comments taken out and each string
  literal put aside (so a comment or a string holding `chord == "` or a
  brace neither counts nor ends the handler early;
  `the_handlers_reading_skips_comments_and_strings` holds a decoy to
  it), fails on any `e.text ==` in it, and
  holds the set of names it compares equal to the table's chords both
  ways; the sheet's rows are checked to carry every line, on Linux and
  on a Mac. Adding a key is a line in `TABLE` and a clause comparing
  its name; forgetting either fails the test.
- The sheet (`ui/panel/keys-sheet.slint`) is a search field over the
  rows, grouped under headings, Ctrl read as Cmd on a Mac and the Mac's
  delete key shown beside Delete (Backspace is bound there alone, which
  the table says, not the handler). ? opens it (from the window's
  keys, so typing ? in the filter types it), Escape or Close closes
  it, and it counts as a sheet: the window's keys are nobody's under
  it. The two pointer gestures (Ctrl+click, Shift+click) are listed
  under Moving around, since a Lightroom hand looks for them there; they
  are the cells' own, not the handler's, and the test leaves them out.
- **Help** sits at the left pane's foot between Settings and Report a
  problem, a menu of **Keys (?)**, **User guide** (the guide at this
  build's tag, `blob/v<version>/docs/user-guide.md`, in the browser off
  the event loop, never from a test) and **Report a problem...**. There
  is no menu bar to put it in; the foot is where the guide has always
  sent people for the report, which keeps its own button.
- The guide's Keys chapter is regrouped under the sheet's headings and
  says where the sheet is.
- The rotate tooltips: the ROTATE section's icon buttons were "Turn
  left" and "Turn right", the same words as the frame's own turn below
  them, which is not part of the edit (§131). They are now "Rotate
  left: part of the edit" and "Rotate right: part of the edit"
  (accessible labels Rotate left and right); the frame-turn buttons
  there and in CULLING say "Turn the frame left ([): not part of the
  edit".

`--sheet keys` and `--sheet help` open the sheet and the menu for a
capture.

Tests: the completeness test above; `a_press_is_named_as_the_table_names_it`;
`the_sheet_says_the_platforms_modifier`;
`a_search_keeps_the_lines_that_hold_it_under_their_headings`;
`question_mark_opens_the_sheet_and_escape_closes_it` (on a bare test
window, the keys under the sheet nobody's, G the window's again after);
`help_has_the_keys_and_the_guide_and_a_test_opens_no_browser`. The
whole window suite passed over the rewritten handler with two tests
changed back by keeping `meta-key`'s text.

## Library and Develop

Two buttons at the top of the left pane, **Library** and **Develop**,
the one on screen pressed (the Cull button's style), setting
`grid-open` as G does. They sit above what the pane scrolls, so they
are in the same place in the grid and the loupe; the loupe has no
header of its own, and the pane's top left is where an eye starts. The
grid header is unchanged, its Loupe button kept, so nothing on it moved
at 1500 or 1280 px (the captures match §339's). The lightbox puts the
panes away and with them the two; L and Escape are its way out. With
the pane brought back in the lightbox (F7), the first cut's Develop
set `grid-open` alone and opened the lightbox's own loupe, still in the
lightbox; that was not meant. Both buttons now leave the lightbox
first, as its own ways out do (`lightbox-leave`, the panes as they
were), Library for the grid and Develop for the frame.

A first run (no settings file yet, not a batch) puts the way round on
the status line: "Library and Develop, top left (or G), switch between
the grid and one frame; ? lists the keys". A frame opening replaces it
with the develop's words, as any status does.

Test: `library_and_develop_switch_the_view_and_the_one_on_screen_is_pressed`
(clicked both ways, the pressed state read off the accessibility tree,
the same place in both views, G agreeing, and both out of the lightbox).

## What is left

- A paste's shift (above).
- Rebindable keys, on the roadmap's pool, now have a table to bind.
- The keys inside sheets and widgets (a slider's arrows, a sheet's
  Return) are not on the sheet: they are the widget's, not the
  window's.
- The first-run hint is not tested: it is written in `startup::main`,
  which no test runs.
