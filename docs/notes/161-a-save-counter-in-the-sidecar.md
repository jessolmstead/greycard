# 161. A save counter in the sidecar (2026-09-24)

Roadmap v0.2.0's line from §153, built in wave B by a sonnet author and
read twice by an opus reviewer.

`Sidecar::find` used to choose between a beside copy and a `.greycard/`
copy by comparing the two files' mtimes, beside on a tie. That is
wrong whenever the files' timestamps do not reflect which one was
written more recently by greycard itself: a copy tool that preserves
mtimes, a backup restore that sets both to the restore time, or two
machines whose clocks disagree. The wrong choice loses an edit
outright, since the next save on the chosen copy settles the pair and
removes the other.

`Sidecar` now carries `saved: u64`. `Sidecar::save` and
`Sidecar::save_in` increment it before writing, by a saturating add
so the field cannot itself be the thing that fails a save; that is
why both moved from `&self` to `&mut self`. Every caller but one
already held its sidecar mutably (they call `.record()` on it in the
same breath); `crates/greycard-ui/src/panel/prefs.rs`'s
`a_stale_copy_removed_is_not_counted_as_moved` test needed a `let mut`
added, the only call site that did not already have one.

The rule as it now stands, in `Sidecar::find`: read both files'
mtimes first, as before, since that costs nothing beyond the metadata
call already being made. Only when both places have a file does
either sidecar's `saved` get read — through a private `SavedCount`
struct of one field, so `find` does not walk the edit, the history
and the snapshots into full `Edit`s and `Vec`s a second time just to
compare two counters (`serde_json` still scans every byte of the file
to know what to skip; this only avoids building the rest of the
`Sidecar`, not reading past it). The one-copy case, the common one,
reads no counter at all: `find` returns on `(Some(_), None)` /
`(None, Some(_))` before `saved_count` is ever called.

The counter decides only when *both* copies carry a nonzero count,
and they differ; the higher one wins. Every other case — either
count 0, or both equal and nonzero — falls back to mtime, beside on
an mtime tie too, exactly the old rule. This is narrower than "higher
count wins" outright, and had to be: a copy with no count is not
necessarily behind. 0.1.1 does not know this field, so a sidecar it
saves comes back out with no `saved` key at all — serde drops what
that build does not know when it writes, same as when it reads — even
if the copy it started from had a real count on it. Concretely: this
build saves a frame under the folder three times (`saved: 3`); 0.1.1
opens the shoot (it reads beside only, per its own placement rule),
finds nothing there, starts from the default edit, and saves beside
with no counter and a newer mtime. Under "higher count wins
outright" this build's `find` would prefer the stale folder copy
(count 3) over the beside copy that is actually the latest edit
(count absent, read as 0) — the same class of bug this field exists
to fix, just moved rather than removed. Treating any 0 as "uncounted,
decide by mtime instead" closes that: the beside copy wins on its
newer mtime, as it must.

`saved` is read loosely, the way `Sidecar::turn`, `Sidecar::meta` and
`Sidecar::xmp` already are (`deserialize_with = "meta::loose"`): a
value that will not fit — `"saved":-1`, say — reads as 0 rather than
failing the whole sidecar, the same as `"turn":"x"` already does not
fail a load. Before this, a strict `u64` meant a single bad `saved`
would have refused the entire sidecar, and the editor would then have
started the frame over from the default edit and overwritten the real
one on the next save — the one thing this feature exists to prevent,
caused by the feature itself. The cheap `SavedCount` reader used
inside `find` tolerates the same case a different way: a `saved` that
will not parse fails that struct's whole (one-field) deserialize, and
`saved_count` treats any such failure as 0, which is exactly the
"uncounted" case `find` already treats specially.

`Sidecar::settle` (the Settings sheet's move action) was already
built on `Sidecar::find` — it renames or drops the other copy
depending on which one `find` names — so it inherits the rule for
free; the two were never at risk of disagreeing, and a test
(`settle_agrees_with_find_when_the_count_beats_mtime`) pins that down
for a case where mtime and the counter disagree.

Tested in `crates/greycard-edit/src/lib.rs`:
- `saving_three_times_counts_three` — three saves reads back as 3.
- `find_prefers_the_higher_count_over_a_newer_mtime` — both copies
  counted, the higher one wins even with the older mtime.
- `find_falls_back_to_mtime_when_counts_are_equal` — equal nonzero
  counts fall back to mtime.
- `find_treats_an_uncounted_copy_as_mtime_only_however_high_the_other_counts`
  — the 0.1.1 scenario above: one copy counted, the other not: mtime
  decides regardless of which count is higher.
- `a_sidecar_without_the_saved_field_loads_as_zero` — a sidecar JSON
  with no `saved` key loads with 0.
- `a_saved_field_that_will_not_parse_costs_that_field_and_nothing_else`
  — `"saved":-1` loads the rest of the sidecar and reads as 0.
- `settle_agrees_with_find_when_the_count_beats_mtime` — settle picks
  the same copy `find` would.

The existing `find_reads_both_places_and_takes_the_newer` test needed
no logic change (its fixtures never set `saved`, so both copies read
0 and the mtime fallback it already exercised still applies
unchanged), but its comment was edited to say why: "both there,
neither with a `saved` count (both read as 0, an equal tie): mtime
decides" in place of the old "both there: the one written last".
`settle_moves_the_newer_copy_and_drops_the_stale_one` needed no
change at all, same reason.

Checked: `crates/greycard-ui/src/cull.rs`'s `move_rejects` only calls
`Sidecar::find` to locate a sidecar to move, never `save`/`save_in`
with the old `&self` signature, so it needed no change.
`crates/greycard-library/src/index.rs`'s meta read
(`sidecar_of`/`read_meta`/`meta_from_json`) reads a sidecar's bytes
through `Sidecar::find` and then its own `serde_json::Value`, same
shape as the new `saved_count` helper; it is untouched and needs
none, since it does not care what `saved` says.

The user guide's sidecar paragraph does not describe the mtime rule
(it says only "a sidecar in either place is read"), so it needed no
change.

What is left: nothing expected.

**The review.** The reviewer built v0.1.1 and master in scratch
worktrees and opened a sidecar this build wrote with each, which is
how the compatibility claim was proven rather than assumed. It then
found the rule's first cut ("higher count wins") had moved the bug
rather than removed it: a copy 0.1.1 saved carries no count, reads as
0, and would have lost to a stale counted copy however new it was.
The rule above is the second cut. It also found `saved` read strictly
where its neighbors are read loosely, so a hand-edited `-1` would have
cost the whole edit. One limit it named and this section keeps: when
both copies are counted but their histories split, the same shoot
edited on two machines, the higher count means more saves, not the
later one, and no rule in the file can settle that.
