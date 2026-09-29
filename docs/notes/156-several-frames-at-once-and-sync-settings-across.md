# 156. Several frames at once, and sync settings across them (2026-09-23)

Roadmap v0.7.0's "Sync settings across a selection" and the backlog's
"Select multiple photos for copying settings"; it also gives v0.8.0's
merge action the set it needs. Wave A, an opus author and an opus
reviewer.

**The set and the anchor.**

The selection is a set of frames with one frame current. The current
frame is `State::current`, as before: the frame the viewport shows,
the frame the panel edits, the row `selected` points at, and the
frame a sync copies from. The rest of the set is
`State::picked: Vec<usize>`, stored as files and not rows, because a
row is only a file's position under the filter and the filter can
move it. The set is always read through `selection::frames(picked,
current)`, which adds the current frame back. So the invariant (the
current frame is in the set, and the set is empty only when nothing
is open) holds even when a code path forgets `picked`. Every older
path that only sets `current` still gets the right answer: the set is
that frame alone.

The current frame is also the anchor that Shift+click measures a
range from. Keeping the anchor and the current frame as one value
means the selection has one frame of interest, not two. It also makes
Shift+click a pure function of the list and the click. The cost is
that Shift+arrow cannot shrink a range the way it does in a file
manager (see below).

The set arithmetic is in `crates/greycard-ui/src/selection.rs`. It is
pure and tested without a window: `click` (what a click with its
modifiers asks for), `toggle`, `range` (along the rows under the
filter, either direction; from a hidden anchor the range is just the
clicked frame), `union`, `frames`, `prune` (drops what the filter no
longer shows) and `others` (the set minus the current frame, which is
what a sync goes onto).

The set is pruned in `rebuild_browser` whenever the list changes. A
key or a sync must not reach a frame that the filter hides, because
nobody can see that it is chosen. The prune does not rely on
`picked` alone: `browser::chosen_frames`, which every key, the turn
and the sync read, drops any frame without a row. That includes a
current frame the filter has just hidden, which `selection::frames`
would otherwise add back. In the first version `show_set` wrote
`frames()` back into `picked`, so a current frame rejected under "No
rejects" still took the next rating key. Moving rejects and opening
another folder clear the set, since both renumber the files.

The strip and the grid show the set with a new `Thumb::chosen` field.
Each frame in the set gets the selection background and a 1 px
accent-muted border. The current frame gets a 2 px accent border as
well. `show_set` writes only the rows whose flag changed, so a click
in a folder of hundreds touches two rows. The grid header shows the
count as "066A3439.CR3 and 2 others".

**Bindings, and why.**

- **Click** opens the frame and collapses the set to it. A plain
  click on the frame that is already current collapses the set
  without opening the frame again, so the second click of the grid's
  double-click does not redevelop.
- **Ctrl+click** toggles a frame in or out of the set. **Shift+click**
  makes the set the range from the current frame to the clicked one.
  **Ctrl+Shift+click** adds that range to the set. None of the three
  moves the current frame. This follows Lightroom's model: the active
  photo is the one you started from, and adding others does not
  change it. Sync copies from the current frame, and the workflow is
  "edit one, add the rest, sync". If Ctrl+click made the clicked frame
  current, the source would move under the user's hand and every
  click would start a develop. The current frame cannot be toggled
  out of the set. To change the source, click it plainly.
- `clicked` in Slint carries no modifiers, so each cell's TouchArea
  records Ctrl and Shift in `pointer-event` on the press, and the
  click reads them. The new callback is `frame-clicked(row, ctrl,
  shift)`. `select(row)` stays as the "open and collapse" entry that
  the rest of the code invokes.
- **Arrows** (strip and grid) move the current frame and collapse
  the set. **Shift+arrow** moves the current frame and adds the
  landed frame to the set, keeping the rest. This is additive, not a
  file-manager range: Shift+Right then Shift+Left leaves both frames
  chosen. The reason is the one-anchor model above. A plain arrow at
  the end of the strip, where nothing moves, still collapses the set.
  `step` and `grid-step` gained a `bool` for Shift, and the opening
  code moved out of the `on_select` closure into
  `browser::open_row(st, app, worker, row, extend)`, so a
  Shift+arrow opens without collapsing.
- **Escape** with a set of two or more collapses to the current
  frame. It sits after the sheet, tool and placing Escapes and before
  the grid's and culling's. In the grid, the first Escape collapses
  the set and the second leaves the grid. Leaving the grid or culling
  with the set still held would leave frames chosen where they cannot
  be seen. The sync sheet's own Escape sits with the other four
  sheets', ahead of the tools'. In the first version it came after
  them, so one Escape with the level tool held and the sheet open put
  the tool down behind the sheet.
- **Ctrl+Shift+S** and the **Sync…** button in the PRESETS section,
  now in the left pane (a row of its own under Import and Save, as
  the pane is too narrow for three; enabled when the set has two or more
  frames) open the sync sheet. Ctrl+Shift+S is Lightroom's Sync
  Settings key. The bare S is the soft proof, and it checks
  `!control`, so the two cannot collide. Neither opens the sheet over
  a tool in hand (a shape being placed, a dropper, a guide, the
  level). The window has a `tool-in-hand()` for the key, and the Rust
  handler checks the same four properties for the button.
- **Keys under a sheet.** The arrows, Space, Z, S and J now return
  early when any sheet is open, and Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y
  do nothing under one. Before this, only a sheet whose LineEdit took
  the focus (the preset sheet) was safe. The review found that with
  the sync sheet open, Right moved the current frame and collapsed
  the set while the sheet still named the old frames. Shift+Right
  then Apply synced frame 2's edit onto frame 1, and an undo under the
  sheet would have synced the undone state. As a second guard,
  opening the sheet records `(current, targets)` in
  `State::sync_asked`. Apply compares that with the selection as it
  is now and refuses with "the selection changed while the sheet was
  open; nothing synced" if the two differ.

**What acts on the set.**

- The meta keys (rating, flag, label, reject) act on the whole set
  through `set_meta`, which already took a slice. A label is still
  settled against the set first. Tested: a Shift+arrow set of three
  given "4" rates exactly those three.
- The frame turn ([ and ]) acts on the whole set too. `turn_frames`
  already looped, and it is the same kind of key as a rating: a fact
  about the frame, not an edit.
- **Export works on the current frame only.** `Job::Export` exports
  the frame the worker has open and has no source path. Exporting a
  set would mean decoding and developing every other frame on the
  worker, with a queue, progress reporting and a naming policy. That
  is a feature, not a small change. The user guide says so.
- Culling's move-rejects is unchanged. It acts on the flags, not the
  selection, and clears the set because the files are renumbered.
- A preset click still goes onto the current frame only. Laying a
  preset over a set is a small follow-up, since `sync_into` is that
  loop.

**Sync, and how it reuses the preset path.**

`greycard_edit::sync_into(sidecars, from, frames, sections)` builds
`Preset::from_edit("", from, sections)` and, for each frame, records
`preset.applied(&sidecar.current)` as one history step. A sync is a
preset that is never written to disk. `Section::copy` is still the
only definition of what a section carries; the adjustments, for
example, come across with fresh ids as a preset's do. There is one
deliberate difference. When Noise is chosen, a sync also carries the
learned denoiser's tier and blend (`greycard_edit::sync_learned`).
A preset's Noise leaves those to each file's ISO, and that behavior
is unchanged. A sync is between frames of one shoot, though, and a
frame taken to the learned tier is the one the others should look
like. With Noise ticked on the sheet and these fields left behind,
the targets would stay off the learned tier while the sheet said
Noise had gone across. A frame that already has every chosen section
records nothing, is left out of the result, and its sidecar is not
rewritten. The meta, the turn and the snapshots are not touched. The
step the sync adds to each frame's history is named by what changed,
by the same `describe` that names a preset's step.

In the UI, `panel::sync::sync_selection` works in this order:

1. It takes the source edit. Outside culling, that is whatever the
   panel holds, recorded and saved on the current frame first
   (`save_edit`), which is what a preset's apply does. In culling it
   is the sidecar's current state.
2. It runs `migrate_frame` on each target, as a turn does.
3. It reads each target's make, model and ISO with
   `greycard_core::decode::probe_path`, a metadata read with no pixel
   decoded. It does this only when one of the next two steps needs
   it.
4. **Camera profile.** When Camera is chosen and the source names a
   DCP, a target the DCP was not made for keeps its own profile and
   takes every other section. `profile_fits` uses `camera::Entry::fits`,
   the same check behind the panel's "Made for X, not Y" warning.
   `Camera::profile()` loads a named DCP with no fit check, so before
   this change a default sync of a set with a Canon DCP developed a
   Fuji frame through the Canon matrices. A DCP the directory has
   not got, or one that names no camera, cannot be checked and goes,
   as it would in a preset. A frame whose metadata will not read does
   not get a camera-specific DCP. The status line names the frames
   the profile skipped: "…; the camera profile left off 4Z4A3521.CR3,
   DSCF0438.RAF (made for another camera)". This was chosen over
   unticking Camera by default. Unticking would stop a set from one
   body getting the DCP it was meant to have, just to protect a
   mixed set that the per-frame check handles anyway.
5. **Learned-blend seed.** A target that has never been opened still
   has `seed_blend` set: it is waiting to be given its ISO's learned
   blend on first open. If Noise is not being synced, the blend is
   written into the target's current edit before the sync records
   its step, and `seed_blend` is cleared. The history's earlier state
   then carries the seeded blend, as it would after an open. If Noise
   is being synced, the blend is the source's, and `seed_blend` is
   cleared so a first open does not seed over it. Without this, the
   next launch's `files::never_developed` would see the sync's
   history, skip the seed, and leave the target at 1.0 rather than
   its ISO's blend.
6. It calls `sync_into`, once for the targets that fit and once, with
   Camera taken out, for those that do not.
7. For each frame that moved or was seeded, it runs `write_sidecar`.
   That calls `save_in` with `st.placement`, so each sidecar goes
   beside the frame or under `.greycard/` as the setting says, plus
   the XMP when that is on. It then updates the row's thumbnail and
   badges.

The probe function is passed in as an argument, so the tests can
fake bodies and ISOs without real raws.

The current frame's edit is not changed. Thumbnails are the camera's
own JPEGs turned by the geometry, and geometry does not sync, so the
pictures look the same after a sync. They are updated anyway, so a
future section that does move them is covered.

**Which sections sync by default, and why.**

`Section::syncs_by_default` covers everything except Adjustments.
This differs from `by_default` for presets, which also leaves out
White balance, Lens, Demosaic and Camera profile as "one picture's
own". A sync is between frames of one shoot: the same body, usually
the same lens, the same light. Carrying the white balance, the lens
corrections and the camera profile is the reason to sync. Lightroom's
Sync dialog also ticks white balance by default.

- **Adjustments (masks)** are unticked because a mask is drawn around
  one picture's subject. On the next frame it lands on nothing, or on
  the wrong thing. Ticking it works, and the masks come across with
  new ids, exactly as a preset's do. They replace each target's own
  masks rather than adding to them, and the sheet's row says so:
  "Adjustments (replaces masks)".
- **Camera profile** is ticked, but it only reaches frames of the
  body the DCP was made for (step 4 above). The sheet's note says so.
- **Noise** is ticked and brings the learned denoiser with it (see
  above). The sheet's note says so.
- **The geometry (crop, straighten, keystone) and the retouch** are
  not offered at all. They are not `Section`s, and presets never
  carried them either. A crop is in each frame's own source
  coordinates, turned by each frame's own turn and orientation. A
  landscape crop pasted onto a portrait frame is wrong, and a
  retouch patch is placed on one picture's dust or blemish. Offering
  either needs a geometry section that maps through each frame's
  aspect and turn (the machinery `turn_by` already has for masks).
  That is left as a follow-up, not added unticked.

The sheet (`ui/panel/sync-sheet.slint`) is the preset sheet's twin:
a toggle for each `Section::ALL` entry in panel order, one line
saying "From IMG onto N other frames", and Cancel and Apply. Escape
closes it. The choice is not remembered between openings: each
opening starts from the defaults.

Both sheets now lay out their toggles in two columns through a
shared `ui/panel/section-toggles.slint`, filled down the first column
and then the second so the panel order still reads top to bottom. The
sheets are 560 wide. Eighteen toggles in one column made the sync
sheet about 920 px tall with no scroll, so at 1366x768 its title and
buttons were clipped, and the preset sheet already clipped the same
way. At 1366x768 the sync sheet is now about 515 px tall and the
preset sheet about 500, and both fit whole (captures below). The
sheet never opens without targets. Ctrl+Shift+S and the Sync button
leave the status line asking for a selection: "choose the frames to
sync onto: Ctrl+click or Shift+click". The "nothing to sync onto"
check in Apply is a guard that the sheet should never reach.

**Measured.**

Measured on copies of four sample raws from the sample set
(066A3439.CR3 with its existing sidecar as the source, then
3G0A4650, 4Z4A2978 and 4Z4A3521, which had none). The editor ran
under its own headless mutter on Xwayland, with `XDG_CONFIG_HOME`,
`XDG_STATE_HOME` and `XDG_CACHE_HOME` under target/work.

- `--also 1,2 --snapshot`: three frames chosen in the strip, the
  current one ringed, and the PRESETS Sync button enabled. The same
  with `--grid` shows the grid header reading "066A3439.CR3 and 2
  others".
- `--also 1,2 --sheet sync`: the sheet lists 18 sections, 17 on and
  Adjustments off, with "From 066A3439.CR3 onto 2 other frames."
- `--also 1,2 --sheet synced` applies the sync on the sheet's
  defaults in the same turn the sheet opens, so the sheet is never
  drawn. The log says "synced 17 sections onto 2 frames". Two new 8772-byte
  sidecars appeared beside 3G0A4650 and 4Z4A2978, and 4Z4A3521 got
  none. `diff` of `jq -S .current` between the source and each target
  shows one differing line: `noise.learned_strength` (0.35 on the
  source, 1.0 on the targets). That was the first version, whose
  Noise did not carry the learned blend. Now it does (next item). The two
  targets are identical to each other. Each target has one history
  state, the default edit, and the diff from it to the current state
  is the source's exposure +0.40. The source's own sidecar is
  byte-identical to before (`cmp`).
- With `--sidecar-folder --also 3`, the target's sidecar was written
  to `shoot/.greycard/4Z4A3521.CR3.gcd`, with the same one-line
  difference.
- Timing: `sync_selection` onto three frames (three sidecar writes)
  took 1.1 ms, logged with the status line.
- **Mixed bodies, after the review fixes.** The shoot was:
  - 066A3439.CR3 (Canon EOS R5), the source. Its sidecar was edited to
    name `canon-r5`, a DCP written for the test with a small script (one
    matrix, D65, UniqueCameraModel "Canon EOS R5") under a
    `XDG_DATA_HOME` in target/work. The sidecar was also set to
    `learned: balanced`, `learned_strength: 0.6`.
  - 4Z4A3521.CR3, which exiv2 reads as "Canon EOS R5m2".
  - DSCF0438.RAF, a GFX100S II.

  `--also 1,2 --sheet synced` logged "synced 17 sections onto 2
  frames; the camera profile left off 4Z4A3521.CR3, DSCF0438.RAF
  (made for another camera) in 1.9 ms". The 1.9 ms includes both
  metadata probes, with the files already in the page cache. The
  second review measured 18.9 ms cold, with the 85 MB RAF read from
  disk, and 2.0 ms warm, so the probe's cost is the first read of
  each file's header. Both targets' sidecars read
  `camera.profile: "embedded"`, `learned: "balanced"`,
  `learned_strength: 0.6` and exposure +0.40. An R5 Mark II is
  another body to a profile made for an R5: the fit check is
  equality, not containment.
- At 1366x768 (`target/work/sheet-small.png`, `preset-small.png`)
  both sheets are drawn whole, title to buttons, in two columns.

**Tests added.**

- `greycard-edit`: `a_sync_lays_the_chosen_sections_over_the_set_as_one_step_each`.
  Chosen sections are copied. Sharpen, geometry, adjustments and
  retouch stay the frame's own. There is one history step, and undo
  takes it back whole. Meta and turn are untouched, a frame outside
  the set is untouched, and an index past the end is skipped. A
  second sync records nothing. Masks, when chosen, come across with
  fresh ids. An empty choice does nothing. A sync's Noise carries
  the learned tier and blend, a preset of the same section does not,
  and without Noise chosen the learned tier stays the frame's.
- `selection.rs`: four tests of the arithmetic (click dispatch;
  toggle, including that the current frame cannot be toggled out;
  range under a filter and from a hidden anchor; frames, others and
  prune).
- `panel/sync.rs`, on the Slint testing backend:
  - `ctrl_click_grows_the_set_and_the_sync_sheet_opens_on_it`: real
    pointer events on the strip with Ctrl held. The set grows, the
    current frame does not move, and the rows' `chosen` flags are
    right. Ctrl+Shift+S does nothing with one frame and opens the
    sheet with three, with the defaults and the words. The first
    Escape closes the sheet and the second collapses the set.
  - `shift_click_and_shift_arrows_extend_and_a_plain_arrow_collapses`:
    Shift+click range, plain click on the current frame collapses
    without reopening, Shift+Right twice extends, the "4" key rates
    the whole set, and a plain arrow collapses.
  - `apply_lays_the_current_frame_over_the_others_and_saves_them`:
    the panel's exposure goes to the other frame in the set, its crop
    is kept, it gets one history step, the sidecar is written under
    the `Folder` placement and reads back the same, and the frame
    outside the set is untouched.
  - `a_profile_fits_only_the_body_it_was_made_for`: the pure fit
    check.
  - `a_profile_skips_another_body_and_a_waiting_blend_is_seeded`:
    with faked bodies and ISOs, a DCP for a Canon R5 goes to the R5
    frame and not the Fuji frame, and Light goes to both. The status
    names the skipped frame. The waiting blends are seeded from each
    frame's ISO under the sync's step when Noise is not synced, and
    are the source's with the seed cleared when it is.
  - `nothing_moves_the_selection_under_the_sheet_and_a_moved_one_is_refused`:
    under the open sheet, Right, Shift+Right, Ctrl+Z and S change
    nothing (undo is never called and the proof stays off). An Apply
    after the set moved is refused, and nothing is written. The sheet
    does not open over the level tool. Escape closes the sheet and
    leaves the tool behind it in hand.
  - `a_hidden_current_frame_takes_no_key`: a current frame the filter
    has hidden is not in `chosen_frames`.
- `Shown::sheet` accepts `sync` and `synced`.

**Snapshot flags.**

`--also ROWS` (hidden) Ctrl+clicks those rows once the first frame
opens. `--sheet sync` opens the sync sheet. `--sheet synced` applies
the sync on the sheet's defaults in the same turn, so the sheet is
never drawn. No key can be sent to a headless mutter
from here, so these are the only way to capture a set, the sheet or
the result of a sync.

**Left.**

- **The learned-blend seed on a picture that is not a raw, or a raw
  whose metadata will not read.** The sync seeds from the probe (step
  5). A file the probe cannot read keeps `seed_blend` for the
  session, so it is still seeded if it is opened before the editor
  quits. A later launch sees its history and skips the seed, as
  before the fix. A sidecar flag for "blend seeded" would close that
  gap too. Not added for so rare a case.
- **Merge action** (v0.8.0): the set is now there for it to read
  (`chosen_frames`).
- **Copy and paste of settings from a right-click menu.** This is the
  same `Preset::from_edit` and `apply` with a clipboard in between.
  There is no context menu on the strip or the grid yet.
- **Preset onto a set.** A click on a preset still goes onto the
  current frame only. It is a small change using `sync_into`'s loop.
- **Export of a set**, for the reasons above.
- **Geometry sync**, which needs mapping through each frame's aspect
  and turn.
- **Remembering the sheet's last choice** for the session.
- **Shift+arrow as a file-manager range** that shrinks when you turn
  back. This would need a second, fixed anchor apart from the current
  frame.
- **Mac key mapping.** On a Mac the binding is Ctrl+click, not
  Cmd+click, which is consistent with the rest of the keys for now
  (user guide).

**The review.** The first cut's sheet was not safe against the keys
behind it: the preset sheet had only ever been safe because its
LineEdit took the focus, and the sync sheet has no field. With the
sheet open, Right moved the current frame and collapsed the set while
the sheet still named the old frames; Shift+Right then Apply synced
frame 2's edit onto frame 1, and Ctrl+Z under the sheet would have
synced the undone state. The reviewer also found that Camera synced
by default with no fit check, so a mixed set developed a Fuji frame
through a Canon DCP; that Noise ticked on the sheet left the learned
denoiser behind; that ticking Adjustments replaced the targets' masks
without saying so; that the sheet's Escape sat behind the tools'; that
the seed of the learned blend was lost on a synced frame first opened
in a later session; and that a hidden current frame still took a
rating key. Every one is above as the design now is. The second pass
reran each probe, repeated the mixed-bodies run, opened a synced
target in a fresh session to see its blend was the ISO's and not 1.0,
and pixel-checked both sheets at 1366x768.
