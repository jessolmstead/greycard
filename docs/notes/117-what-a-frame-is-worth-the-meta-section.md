# 117. What a frame is worth: the meta section (2026-09-20)

The sidecar holds one more thing now: what a frame is worth and what
it is called. `greycard-edit`'s `meta` module is a rating of 0 to 5, a
pick-or-reject flag, a color label out of Lightroom's five, keywords,
a title and a caption, and `Sidecar` carries a `Meta` beside its
`current`, its `history` and its snapshots.

Beside, not inside, and the distinction is the whole point. A rating
is a judgment about a picture, not a step in developing one. Put it
in the `Edit` and Ctrl+Z takes a star back, a preset carries one
frame's four stars onto the next, and a snapshot restores the opinion
you had on Tuesday along with the exposure. So `Meta` is its own
field on the sidecar: `record`, `undo`, `redo`, `go_to` and
`restore_snapshot` never touch it, setting it records no state and
marks nothing dirty, and `Edit::to_json` has no "meta" in it at all,
which is what keeps a preset honest. The test
`undo_after_a_rating_takes_back_the_edit_and_not_the_rating` is the
one that says so.

One file, though, not two. §72 says a directory is the library and
the file beside the frame is all there is; a second file beside it
for the meta would be two things to keep in step, two writes to get
right, and two ways for a folder copied somewhere to arrive half
itself. So the `.gcd` holds both, and either half may be missing: a
sidecar with a meta and no edit opens the frame at the default edit
(rate a shoot before developing a frame of it), and every sidecar
ever written before today has an edit and no meta, and loads
unchanged. A meta that says nothing is not written at all, so
developing a frame nobody has rated does not grow an empty block.

No schema bump. `VERSION` stays 3. The rule in `edit/lib.rs` is that
a field whose *meaning* changes bumps it and gets a migration; a new
section with a default does not, because `#[serde(default)]` already
reads the old file and `Error::Newer` is a refusal, not a courtesy —
bumping would make every sidecar this build writes unreadable to the
build before it, in exchange for nothing.

What pays for having no version is reading the section loosely.
Every field of it falls back to its default rather than failing: an
unknown label (a sixth color a later build writes), a flag spelled by
hand, a rating of two hundred, a keyword list that is one word rather
than a list, and the whole section when that is not a section either.
The reason is that the meta shares a file with the edit and the
history: a sidecar refused over one misspelled word would open the
frame as a fresh one and let the next slider write over a year of
work. Forward compatibility here is not politeness, it is the
difference between a badge going missing and an edit going missing.

The keys are the ones a culler already has in their hands: 1 to 5 the
stars, 0 none, P a pick, X a reject, U neither, and 6 to 9 red,
yellow, green and blue. Purple is in the schema and has no key, the
same bargain Lightroom makes: the sixth label would have to take 0,
and clearing a rating is worth more than a sixth color.

The label key toggles — pressed again it clears — and the rating and
flag keys do not: 1 to 5 set, and 0 and U are how you take them off.
That differs from Lightroom Classic, where pressing a rating key
again clears it. The reason is key repeat: a cull is a key held or
struck twice in a hurry, and a toggling rating flips between four
stars and none under exactly the pressure that makes the toggle
convenient, while a label is pressed once and looked at. The toggle
is settled across the whole selection rather than per frame —
`Change::settled` clears when every frame in the set already carries
the label and sets otherwise — because deciding frame by frame would
leave a mixed selection half red and half bare, which is neither
thing the press could have meant.

There is no multi-select in the browser yet, so the keys act on the
selected frame. The function they call, `set_meta`, takes a set of
frames, and `meta_into` settles and applies over that set; when the
browser grows a multi-select, it is already what it calls. The write
is prompt and goes through the same `Sidecar::save` the edit uses —
one file, one struct, so the meta cannot clobber an edit nor an edit
the meta. What is on the panel may be newer than what is on disk when
a star is pressed; the save timer writes that a moment later with the
meta beside it.

The keys work in the loupe as well as the grid. The filmstrip is the
browser, and a culling pass that stops working the moment you look at
a picture properly is not a culling pass. Nothing in the set is
modified, so no Ctrl or Alt shortcut is eaten, and a sheet over the
window keeps its own keys. There is no word in the status line to go
with the key: the next thing a cull does is the arrow to the next
frame, and that frame's develop would write over any such word before
it had been read. The badge is the answer, and it is on screen in
both views.

The badges are one small pill in the bottom left corner of the
picture: the flag, then as many stars as the rating is, then a dot in
the label's color, over a scrim dark enough to read against a white
sky. As many stars as it is, rather than five with some filled, so a
two-star frame is a quieter mark than a four-star one and a frame
nobody has said anything about shows nothing at all — a browser is
for looking at pictures. On the picture, not in the cell: `contain`
leaves a margin beside a portrait frame, and a badge in that margin
reads as sitting beside the picture rather than on it, so the grid
works the rendered box out from the thumbnail's aspect. It is held
inside the cell all the same, since at the smallest cell a five-star
badge is wider than a portrait frame. The glyphs grow with the cell
between 9 and 18 pixels: the smallest cell keeps the size the strip
uses, and the largest does not turn a quiet mark into a caption.

One thing the meta broke on the way in, and it is worth remembering
because the shape of it will come back. The learned denoiser's blend
is seeded from the ISO on a raw's first open, and what decided
"first" was whether a sidecar existed at all. A star in the browser
writes a sidecar, so a frame rated before it was ever developed
opened at a blend of 100% instead of the 35% its ISO asked for. The
fix is to ask the sidecar what is in it — a default edit, no history,
no snapshots — rather than whether it is there. Any state that used
to mean "nobody has touched this file" has to be re-read now that
something other than a develop writes beside the frame.

Left out, and on the roadmap rather than forgotten: filters (show me
the picks, hide the rejects), XMP so Lightroom and darktable read the
same ratings, a CLI that lists or sets them, and a panel for the
title, the caption and the keywords — the fields are in the schema
and round-trip, but nothing in the window edits them yet.
