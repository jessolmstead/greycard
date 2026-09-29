# 204. A Background shape (2026-09-29)

Roadmap line: "A Background shape: the Subject mask inverted, one
button beside Subject, so a look goes on everything but the subject
without drawing it; later the parts models make it the true
background rather than the subject's complement."

**What it is.** `Shape::Background {}` in greycard-edit's mask module,
beside `Subject {}`: the tag is `background`, learned and a raster
shape exactly as Subject is, so the panel's shape list, the CPU
composer and the GPU shader all treat it as any other model-made
shape — none of them has a line of Background-specific code, because
a raster shape only ever needs *a* raster handed to it by index; what
that raster holds is `greycard-ui`'s business. It is a kind of its
own rather than a Subject with `invert` set, on purpose: a sidecar
says plainly which a shape is, and the day a parts model finds the
true background — the room behind a portrait's subject, not merely
what the subject model missed — filling `Shape::Background`'s raster
differently needs no schema change and no sidecar migration.

**How it is made.** `greycard-ui::ai::Ai` keeps the Subject matte it
last made, at the raster's size, beside the base develop `stamp` it
was made for and the provider that made it (`subject_matte:
Option<(u64, Vec<u8>, Provider)>`). Asked for either a Subject or a
Background raster, `Ai::raster` checks that matte before touching the
model store at all; a hit needs no store and loads nothing, and still
returns the provider that made the shared run — so the status line
("background found on ... in 0.00 s") and Show mask react to a shared
hit exactly as they do to an actual run, rather than the status line
that asked for it ("finding the background") being left up forever
the way a plain cache hit (which names no provider) already did before
this, unnoticed until Background gave it a common case to happen in.
On a miss `Ai::raster` runs the Subject model exactly as it always
has (load it if not loaded, `subject.mask`, `refine`, resample to the
raster's size), keeps the result and the provider there, and only then
reads it back for whichever shape asked — a Background's own bytes are
one minus the shared ones, `255 - v` each. So a mask carrying both a
Subject and a Background shape, or two separate masks each with one,
costs the model one call between them, whichever asks first; the
existing per-shape disk cache (`masks/<hash>.png` under the model
store, keyed by the file, the model and the shape) still applies on
top, so a Background made once is not made again next time the file
opens either, the same as Subject already was. The shared matte is
dropped on another file (`Ai::forget`) and on a new Subject model
arriving in the store (`Ai::forget_subject`), since a different model
makes a different matte and neither shape should read a stale one.
`panel::mask::step`, which decides whether a want is asked for, offered
or waited on, now treats Background exactly as Subject for the
fallback to the store's own original when nothing can be offered this
round — it only recognized `Shape::Subject {}` for that before, so a
Background in a mixed mask waited on an offer that was never coming
while the Subject beside it already ran.

**The button.** In the Masks tab's "Add a shape" row (the single one
the layout item above put there), Background sits beside Subject: its
own kind, its own icon (`image.svg`, greycard's stand-in for "the
whole picture" until a background icon of its own is worth adding),
made whole with no drag, exactly as Subject is. `made_at_once` and
`made_hint` both learned the kind ("finding the background" on the
status line while it runs); everywhere else a `Shape` is matched
exhaustively (the placing flow's few catch-all arms, the panel's
handle and box drawing, the GPU's shape upload) took the new variant
into whichever arm Subject already sat in, since none of that code
cares which raster shape it is looking at.

**An old build's sidecar.** `Shape` reads an unknown `kind` as
`Shape::Unknown` through the existing `#[serde(other)]` — the same
fallback any shape kind a build does not recognize already gets:
counted as nothing (not even joined, as a shape switched off is not),
kept through the rest of the edit so nothing else is lost, and
dropped when that build saves the mask again. An old build reading a
sidecar with a Background shape gets exactly that: the mask reads as
if the Background were simply switched off, and a save from that old
build quietly drops it. Nothing new needed checking here — it is the
mechanism `a_shape_from_a_later_build_is_nothing_and_the_rest_is_kept`
already covers for any shape kind a build has not seen, background
included as of this build.

**Checked.** `3G0A4650.CR3` through the editor with two masks, one a
bare Subject, the other a bare Background, both shown in turn
(`--show-mask`, real Subject model on WebGPU): the Subject matte
covers the two chalk cliff masses — the rock and the grass on top of
them alike — and nothing else; the Background matte is their precise
complement, covering the sky, the sea and the grass at the camera's
feet, with the same boundary line either way and neither the cliffs'
rock nor their grassy tops touched. A second run for the Background
alone loaded neither the model nor logged a fresh inference, having
already read both shapes' disk cache from the first run — the
ordinary per-shape cache, working across a Background exactly as it
does across a Subject. Kept under `target/scratch/`: `subject-matte.png`,
`background-matte.png`.

**Tests.** The shape round-trips through a sidecar on its own kind
(`a_background_is_learned_and_round_trips_on_its_own_kind`,
greycard-edit). The composed Background matte is one minus the shared
Subject matte's, byte for byte — the CPU reference — and needs neither
the model store nor a model load once the shared matte is already
there, and still names the provider that made it, proving the
one-call sharing and the status line's fix together without needing
real weights in the test
(`a_background_is_one_minus_the_shared_subject_matte_and_costs_it_nothing`,
greycard-ui). `panel::mask::step` is checked against Background with
the same four cases `a_subject_already_in_the_store_offers_the_rewrite_when_it_falls_back`
already covers for Subject
(`a_background_falls_back_to_the_original_exactly_as_a_subject_does`).
The button appears once, in the "Add a shape" row alongside the rest,
covered by the same layout test the section above added.

**Not done.** The button's icon is a stand-in (`image.svg`); a
purpose-made "background" glyph is a small follow-up, not blocking.
The roadmap line's "later" — a parts model that finds the true
background rather than the subject's complement — is exactly that:
later, and needs no more schema work when it comes, which was the
point of making Background a kind of its own now.
