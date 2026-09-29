# 162. A preset laid over the selection (2026-09-24)

The backlog line §156 left, built in wave B by a sonnet author and read
three times by an opus reviewer.

Roadmap backlog item, following on from §156 (multi-select and sync
settings): a preset click now lays the preset over every selected
frame, not just the one on screen, the same rule whether one frame is
open or a whole set is selected.

**Reusing the sync's loop.** `sync_selection`'s per-target work
(migrate the sidecar, check a named camera profile against each
target's body, seed a waiting learned-denoiser blend, apply, write the
sidecar, refresh the thumbnail and badges) is factored out of
`panel::sync::sync_selection` into `panel::sync::lay_over_targets`,
which takes a `Preset` and a list of target frames rather than
rebuilding one from an `Edit` and a section list internally.
`sync_selection` is now a thin wrapper: it builds the sync's `Preset`
from the current frame and calls `lay_over_targets`.

One thing does not travel through the `Preset` alone: a sync's Noise
carries the learned denoiser's tier and blend along
(`greycard_edit::sync_learned`), and that is not something
`Preset::apply` ever does — those fields are deliberately left out of
`preset.edit` even when the preset was built from `Preset::from_edit`
with Noise chosen, because a preset (saved or not) leaves the learned
tier to each file's own ISO. So `apply_preset_into`, the shared bottom
layer in `greycard-edit`, takes `learned_from: Option<&Edit>` rather
than a bool: `Some(source_edit)` when a sync's Noise is carried,
`None` always for a preset click. `sync_into` (the file-facing
function, used by `sync_selection`) is unchanged in its own signature
and now sits on top of `apply_preset_into`.

**One function, one frame or a set.** What a preset click does —
the current frame's own apply (through the panel outside culling,
through the sidecar under it), the camera-profile fit check, the rest
of the set through `lay_over_targets` when there is one, and the
status line — is `panel::sync::apply_preset(st, app, worker, preset,
profiles, probe)`, pulled out of `panel::assets::on_preset_applied`'s
Slint closure the way `sync_selection` already sits apart from
`on_sync_applied`. The closure itself is three lines: resolve the
clicked preset, read `camera::list()`, call `apply_preset`. Keeping it
as one function (rather than a branch for one frame and a branch for
a set that could drift apart) is what makes the fit check below the
same rule everywhere, not a rule the set happens to add.

**The camera-profile fit check reaches every frame the same way,
current or target.** `panel::sync::preset_for_body(st, preset, file,
profiles, probe)` is `lay_over_targets`'s per-target check, pulled out
as a free function: the preset unchanged when it carries no named
profile or the profile fits the frame's body, a copy with Camera taken
out and `true` when it does not. `apply_preset` runs it once for the
current frame before either branch, so a DCP made for one body never
lands on the frame on screen just because it is the one open — the
same as any other frame in a set. A profile left off this way is still
there to choose by hand from the CAMERA PROFILE section, which warns
"Made for X, not Y" rather than refusing it. A left-off current frame
gets the same status words a left-off target does
(`profile_left_off_words`, shared by `preset_onto_words` for a set and
built from a one-frame `Synced` for a single click): "Portra 400
applied; the camera profile left off IMG_0001.CR3 (made for another
camera)".

**The status line, and surviving the develop it can trigger.** With
one frame the wording is what it always was: "Portra 400 applied" or
"Portra 400 is on already", with the left-off tail above when it
applies. With two or more selected, the line names how many of the
selected frames took it and how many already matched: "Portra 400
onto 3 frames" or "Portra 400 onto 1 frame; 2 had it already". Laying
the preset over the current frame can call `take_current` (or
`leave_cull`, in culling) to develop it, and both set their own
"developing..." status first. Outside culling, when a real develop is
asked for, the words are not set on the spot: `take_current` bumps
`State::generation`, and `apply_preset` stashes them in
`State::status_after_develop: Option<(u64, String)>`, keyed to that
generation. `deliver.rs`'s `Outcome::Developed` arm — the one place
that builds the "WxH, developed in ..." line once a develop lands —
takes the stash when its generation matches and puts the words first,
the develop's own line after: `"Portra 400 onto 3 frames; 1024x683,
developed in 0.8 s"`. First, not last, because the develop's line can
run long on its own (a dehaze note, a sharpen note, the learned
denoiser, a fill) and would otherwise clip the words off the end of a
narrow window or a snapshot. `Option::take_if` leaves a stash whose
generation does not match in place rather than clearing it; that is
never wrong on its own, since generations only rise and such a stash
can never be matched by a later develop either, but `take_current`
also clears `status_after_develop` outright whenever it asks for a
fresh develop, so nothing is left waiting on a generation that will
not come. When the panel's change did not need a develop at all (every
one of `same_develop`'s fields unchanged — a Light-only preset does
not touch them, for one), the words are set directly, as before.
Culling's own status (the placeholder text `cull.rs` puts up) is a
separate, more involved mechanism than `deliver.rs`'s develop-landed
line; the words are not carried through it the same way, and the
comment on that branch says so rather than promising a guarantee it
does not keep.

**The culling save.** In culling, `leave_cull(.., None)` reads the
edit to leave with straight off `st.sidecars[c].current` — the very
sidecar the preset click just recorded onto — so the two are never
different and the write `leave_cull` schedules on a difference never
fires: the step lived only in memory, the `.gcd` never got it. Both of
`apply_preset`'s culling branches (one frame, and the set) call
`write_sidecar` themselves before `leave_cull`, and set the status
line again after it, since `leave_cull` sets its own.

**The CLI.** `preset-onto-set` was added to `panel::viewport::Shown`'s
`--sheet` names (alongside the existing `sync` / `synced`) since
nothing else in the CLI can fire a stored-preset click over a set in a
scripted run (`--preset` bakes a named preset into the first file's
sidecar before the window even opens, bypassing the click handler and
the selection entirely). It shows in `--help` like every other
`--sheet` name; only `--also` (the rows it puts in the set) stays
hidden, as it already was for `--sheet sync`. Verified for real: a
copy of 066A3439.CR3 (with its existing sidecar) plus 3G0A4650,
4Z4A2978 and 4Z4A3521 (no sidecars), opened under the editor's own
headless mutter with `--also 1,2 --sheet preset-onto-set --snapshot`.
The seeded "Muted Slide" preset (alphabetically first) landed on
066A3439 (one more history step, through the panel) and on 3G0A4650
and 4Z4A2978 (one history step each, fresh 8800-byte sidecars,
matching `current.color` in both — saturation -0.45 as Muted Slide
sets it); 4Z4A3521, outside the `--also` set, got no sidecar at all.

**The guide.** "Several frames at once" gets a sentence, after the
Sync settings paragraph it follows on from, saying a preset click
reaches the whole selection, one history step apiece; a second
sentence says a named camera profile reaches only a frame of that
camera regardless of how many are selected, and points at the CAMERA
PROFILE section's warning for choosing it anyway.

**Tests**, all in `panel::sync::tests`:
`a_preset_click_with_three_selected_lays_it_over_each_and_leaves_the_rest`
(real Ctrl+click pointer events pick three frames; a frame already
matching the preset gets no new step; the frame outside the set is
untouched; the two written sidecars round-trip through `Sidecar::load`);
`a_preset_click_with_one_frame_selected_is_unchanged` (the old
"applied" / "is on already" wording, and history length, exactly as
before the set existed); `a_preset_click_checks_every_bodys_fit_the_open_frame_included`
(`preset_for_body` and `lay_over_targets` together, over a mixed-body
set); `apply_preset_names_the_open_frame_among_a_left_off_profile` and
`a_single_frame_click_names_a_left_off_profile_and_the_words_ride_the_develop`
(`apply_preset` itself, one frame and a set, with fake bodies —
neither `camera::list()` nor a real file's metadata can be faked
through the Slint callback, so these call `apply_preset` directly, the
same way `sync_selection`'s own camera-fit test bypasses
`on_sync_applied`); `a_preset_carrying_noise_leaves_the_learned_tier_to_each_frames_iso`
(a store preset's Noise never brings the learned tier the way a sync's
does, and still seeds each waiting frame's blend from its own ISO);
`a_preset_click_in_culling_saves_the_open_frames_sidecar` (both
culling branches actually write the `.gcd`, not just the in-memory
sidecar); `a_multi_frame_presets_words_ride_the_develop_that_lands`
and the single-frame test above (the stash, the generation match, and
the words-first ordering, over a real `Outcome::Developed`, built from
a white balance change since exposure alone never asks the worker for
one).

**The review.** Three passes. The first found the open frame and the
other frames following different camera-profile rules within one
click, the set's status line vanishing behind the develop's, and a
culling path that never wrote the open frame's sidecar, which turned
out to predate the branch. The second found the single-frame click
still without the fit check the set had gained, which is how
`apply_preset` became one function. The third found the single-frame
path silent about a profile it left off. Each was verified in a real
run with a DCP naming another body, and the plain single-frame click
was checked byte for byte against master's sidecar, so the old path
is the old path.
