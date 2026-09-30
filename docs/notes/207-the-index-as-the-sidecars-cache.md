# 207. The index as the sidecars' cache (2026-09-29)

The roadmap's line: a root opens without a read from its disk, an
offline root still shows its grid, greyed, with the filter working, and
only opening a frame needs the disk. §192 said the wait on the NAS was
work and put a bar over it; this takes most of the work away. It is the
leftover §187 and §192 both named last: a folder's own open still read
every sidecar on the window's thread, and a view of the roots read every
sidecar on the pool before it showed anything. One review round, whose
findings are folded in below where they changed the design.

**What it was.** A frame in the browser's list had its `Sidecar` in
memory from the moment it was listed, read from disk: `open_files` read
a folder's on the window's thread, and a view of the roots read the
whole library's on the rayon pool (§174) while the window kept the list
it had. A root that was offline was left out of the view (§174), since
nothing under it would answer. The index's row already mirrored the
sidecar's rating, flag, label and keywords, keyed by the sidecar's hash
and mtime and refreshed by the pass (§160, rule 1), and nothing read
them: the row's meta was for the filter's `rating:` terms and the CLI.

**Now.** A list that came from the index — a view of the roots, or a
folder the index knows — is built from its rows. Each frame's `Sidecar`
in `st.sidecars` is a stand-in: the row's meta, the frame's default edit
(a raw's `Edit::default()`, a picture's `Edit::for_picture()`), nothing
read. Beside it `st.from_row` says the frame stands in, and holds what
the row gives besides the meta: the quarter turns and the mirror the
picture is shown at, and the file's hash and mtime stamp, which is the
thumbnail cache's key (§172's `thumbs.rs`: `<hash>-<size>-r<recipe>-
<stamp>`). The sidecar itself is read the first time something needs
the whole of it. From then on the copy in memory is the one that
counts, as it always was.

**How a sidecar is read once it is wanted: a request and a queue.**
Off the window's thread, and by a design the second review asked for
in place of the first two cuts'. The first cut read a picked frame's
sidecar on the window's thread (three `.gcd` calls there per open
where master had none) and a selection's on the pool with the window
waiting. The second cut read them on threads and *re-invoked the
callback* when they landed (`app.invoke_meta_key(key)` and the like),
dropping any read a later one overtook through one global token. The
review found what that costs: an arrow onto an unloaded frame left the
old frame current, so a key pressed while the read was out went to
both and was saved on both, and the key's own read canceled the
open's, leaving the spinner up; a callback re-invoked acted on the
selection as it was when the read landed, so a preset pressed on fifty
unloaded frames and a click on a loaded one meanwhile put the preset on
the one; two keys during one read lost the first; an open landing
re-ran `open_row` by row number, so a click on A then on B jumped back
to A; a dropped read left the bar up; and Purple and None labels had no
key string to come back through. Every one of them is the same
mistake: the action was parted from what it was pressed on.

Now (`rows::request`, `rows::Loads`): a request is the *paths* an
action was pressed on, captured then, and the action itself, a closure
that is handed, when the sidecars are in, where each of those paths is
in the list now. The action runs on exactly those frames, whatever the
selection is by then; one no longer listed is left out. Requests are a
queue, run in the order they were made; a later request never cancels
an earlier one, and each reads only the paths no earlier request is
already reading, so two keys on one frame during its read are one read
and two actions, in order. A read is a thread over the pool with the
bar over it — state of its own in `State::loads`, beside the roots
view's read and its bar, which a frames' read never touches; the bar
shows only while no view's read has it, and gives up on its own reads
(60 s out, the count still 10 s) by dropping every request queued,
actions and all, and saying which by name: "the open of y.cr3, 2
rating keys and the flag key dropped: the sidecars did not come in". A
dropped pick is let go (not busy, no pick pending; the frame stays
shown as an offline one is, its stand-in on the panel). When the list
is replaced every request is dropped, and a read still out lands
*nothing*: it was started under another list, and the path may have
been written since (a culling control through `control_over_frame`, an
export's record on disk), so its older bytes would come back and the
next key would save them over the newer. A stand-in is filled only by a
read started under its own list (the read carries the
`view_generation`; the third review found the first form of this
taking such a read's sidecars "as good"). A frame a request named that
is gone from the list when the request runs (moved, deleted) takes
nothing of it, and the status line says so after the action has had
its say: "y.cr3 moved or is gone; the rating key was not applied to
it".

Opening a frame is a request too. The moment the user picks a frame
whose sidecar stands in, it is current (`st.current` moves with
`st.picked`), the panel shows the stand-in's default read-only with
"reading X's sidecar..." on the status line, the frame before it is
left as any open leaves it (its panel saved), and `State::pick_pending`
names the pick. The request's action opens the frame only if that path
is still the pick and still current; otherwise the sidecar is in memory
and nothing shows. Never by row number. Keys pressed on the pick
meanwhile are requests on it alone, queued behind its open, so nothing
reaches the frame that was current before the pick. Culling's move-on
steps at the *press*, not when the key lands: the key's request carries
the frame the user saw, and the pick moves to the next frame at once (a
stand-in if unloaded, a request of its own), so a run of keys during a
run of reads rates a run of frames and ends where the user expects. A
key that applies at once (the frame loaded) keeps §123's rule for a
frame the key took out of the filtered list: the nearest frame shown
takes the selection, once, not the one after; a key queued behind a
read steps at the press and, when it lands, may not select again —
except on the last row, where the step had nowhere to go, so the
landing may put the selection on the nearest row shown, as the key
applied at once would have. The fourth review found the end frame
differing by disk speed there: an unflag on the last pick left culling
on a frame the Picks filter now hid. Culling's undo and redo are
requests too, with nothing to read, so they queue behind the keys still
waiting and take back the last key the user *pressed*; run at once,
undo took back the key before the one still queued. The
third review found the step taken when the key landed, from whatever
was current by then: keys 4 then 2 on an unread frame both landed on it
and stepped twice, and ten keys put the last on the first frame and
jumped ten. The callbacks that
were re-invoked are plain requests now: a key (`meta_on_selection`,
carrying the `Change` itself, so Purple and None need no key string), a
turn, the sheet's sync (the source's path first, then the targets') and
paste, a preset (`preset_pressed`, `apply_preset_to` over the pressed
frames, as targets alone when the frame on screen is no longer among
them), and an export (`export_frames` over the pressed frames). In a
test the reads are queued and landed by `rows::land_pending`, as the
roots' reads are; the threads and the landings are exercised by running
the editor.

`rows::load_frame` and `rows::load_frames`, synchronous, stay under the
requests as the safety net and for the paths with no callback: culling's
`cull_select` when reached by a step, the launch's `Tweak`,
`apply_preset_to`'s own frame, `copy_settings_of`.

**The rule for every use of `st.sidecars`**, gone through one by one,
and again by the review, which found three I had missed (`turn_frames`,
`thumb_turns_of`, and the sources below):
- `.meta` may be read from any frame, stand-in or not: the filter
  (`filter_frames`, `filter_shows`), the badges and `thumb_for`, the
  keyword facet's counts and its spelling (`keyword_counts`,
  `label_of`), the reject count (`to_move_out`), the context menu's
  metas, culling's compare tiles.
- The turns a picture is shown at come from the row while the frame
  stands in: `thumb_turns` and culling's `thumb_turns_of` (the compare
  tiles, the loupe's stand-in, the leaving develop's placeholder), which
  now takes `from_row` beside the sidecars. The row carries the turns
  for exactly this; the first cut left `thumb_turns_of` out, and the
  compare tiles drew a turned neighbor unturned.
- Everything else wants the frame loaded first, and loads it:
  - opening a frame (`open_row`, through a request), which the develop
    and the history panel want, so the frame on screen is loaded unless
    its root is offline or its sidecar is still on its way
    (`pick_pending`, when the panel is the stand-in's and read-only):
    the edit panel's saves (`save_edit`, `write_sidecar`,
    `take_sources`), the history panel, snapshots and `current_turn`
    need nothing more;
  - culling reaching a frame (`cull_select`), the leaving develop
    (`leave_cull`) and a control moved in the mode (`control_over_frame`);
  - a rating, flag, label or keyword key, in `set_meta`, since a save
    writes the whole file: the frames are loaded, the change applied,
    the sidecar saved and `index_file` asked as before;
  - a turn (`turn_frames`), which the first cut turned on the stand-in
    in memory: `writable` refused the write without a word, and the
    thumbnail kept the row's turns;
  - the targets of a sync, a paste or a preset (`lay_over_targets`),
    the frames of an export (`set_frames`, now `&mut State`), and the
    clipboard's copy of another frame (`copy_settings_of`);
  - the *source* of a sync, a copy or a preset: `sync_source`,
    `copy_settings_of` and `apply_preset` refuse a frame whose sidecar
    stands in, with a status line. The first cut let culling step onto
    an offline frame and sync from it, which laid the stand-in's
    default edit over every target; and in the loupe the offline frame
    kept the last frame's edit on the panel, so Copy and Sync took that
    frame's edit for this one's.
- A stand-in is never written: `panel::edit::writable` is the one test
  (`write_sidecars`, not being deleted, and not standing in), and every
  write goes through it, `preset_at_start`'s included (the first cut's
  did not; it now gets its placement only when `writable` says so). A
  save of a stand-in would put the default edit over the frame's own. A
  slider moved over a stand-in (an offline frame on screen, a pick
  whose sidecar is still on its way, one whose read failed or was
  given up on) is refused, nothing recorded, and said by
  `rows::not_kept`: "<root> is offline: the change is not kept",
  "y.cr3's sidecar is still being read: the change is not kept". A
  snapshot of a stand-in is refused the same way.
- The renumberings carry `from_row` with `sidecars` and `seed_blend`:
  `open_loaded`, `merge`, the rejects' move.
- Under `--no-sidecars` nothing changes: every frame is held as read
  with the default sidecar and no meta, as `load_sidecar` gave it.

`seed_blend`, which said whether a raw's edit was still the default and
so its learned-denoiser blend was to be seeded from the ISO on first
open (§40), was decided from the sidecar's contents. For a stand-in it
is the row's `edited`, negated.

**The row grew two columns.** `edited`: the sidecar holds a develop — a
step in its history, a snapshot, or a current edit that is not the
frame's default. A frame only rated, or never opened, has none. The
choice is `files::never_developed` as the browser had it, made against
the right default: a picture's default is `Edit::for_picture()`, and a
raw's edit saved onto a picture is a develop of the picture. A `.gcd`
this build cannot read — its edit, or the file itself — counts as
edited, since there is something in it (the first cut counted a file
that was not JSON as not edited, against its own claim; the review
caught it). `turns`: `Geometry::shown_turns` of the edit with the
frame's own turn, packed as the turns plus four when mirrored. Both are
read by the pass from the sidecar it already reads for the meta:
`current` alone is brought up to this build's shape and parsed, the
history and the snapshots are only counted, so the cost on a sidecar
with a long history is one edit's parse and not fifty. `Entry` and the
new `RowMeta` carry them; `rows_under_canonical` lists them under the
roots by folder then name, `rows_of_with` by path.

**The XMP beside the frame.** Master's lists went through
`load_sidecar`, which takes what an XMP another tool wrote has to say
(§133's `xmp::adopt`: a packet the sidecar has not taken from yet, by
its mark, has its rating, flag, label and keywords laid over the meta
and its orientation over the turn). The index read only the `.gcd`, so
the first cut's stand-ins showed a Lightroom folder's ratings as zero
until each frame was opened, and the review's test said so. Now the
pass reads the XMP as the loader does: `sidecar_of` finds the `.gcd`
and the XMP (`xmp::path_of`), folds both into the row's `sidecar_hash`
(the `.gcd`'s bytes as before, then the XMP's length and bytes), and
the summary builds a sidecar of the `.gcd`'s meta, turn, mark and
edit and calls `xmp::adopt` on it, the camera's orientation tag read
only when the packet says something about the orientation, as the
loader reads it. So the row equals what `load_sidecar` gives: an XMP
alone gives its rating; an XMP over a `.gcd` with no mark wins,
whatever the two files' times; one the `.gcd` has taken from already
says nothing more, so a rating cleared here is not undone by an older
opinion; and a changed XMP, or a `.gcd` appearing beside one, is a
change to the hash and the row is written again. A frame with only an
XMP has a `sidecar_hash` and no `sidecar` path, and `same_sidecar`
compares both. The XMP's orientation goes onto the frame's turn and
not into its edit, as in the editor, so it moves the row's `turns` and
not its `edited`: a frame only turned in another tool (tiff:Orientation
6, 8 or 3) counts as not developed, its edit still the default.

Schema 4. Not the rebuild §160 named for an older file but §181's
in-place step: the two columns added, and `sidecar_hash` cleared on
every row with a sidecar, so the next pass over its folder reads the
sidecar again under rule 1 and fills them, and reads nothing else of
the file. A rebuild of a large library is hours; this is a sidecar read
a row. Until that pass a schema 3 row says "not edited", which costs a
raw its ISO seed on its first open from a view if it was developed
elsewhere meanwhile — the same frame opened from its folder, whose pass
runs at once, is right. A schema 3 row of a frame with an XMP and no
`.gcd` has no hash, and the next pass reads the XMP the same way.

**Fast first, right after.** The stand-in is as right as the row, which
is as right as the last pass over the folder or the last save indexed.
So the pass stays the corrector: rule 1 re-reads a sidecar whose hash
or mtime differ and rewrites the row, meta and the two columns, and the
pass's word to the window has the list read again (§187). The read
brings every listed file's row (`Brought::rows`), and the merge takes
each row's meta, seed and turns into the frame that still stands in
(`rows::apply_row`), whether or not the list itself changed — a merge
whose list is the same used to return at once, and a sidecar changed
by another tool never showed. A frame already loaded keeps what memory
holds, as a merge always kept each frame's own by its path. The
window's own read of the ids (`refresh_ids`, after the folder's pass
and every word from the indexer) does the same for a folder's list,
reading the rows of the frames still standing in and only the ids of
the rest, so a folder's open on a stale index is corrected the moment
its pass reports; a view of the roots has its rows brought by the
merge its pass starts, off the window's thread.

**An offline root** (§174) is in the view. Its frames come from their
rows like the others; its folders are not looked at (nothing there
would answer, and a folder that cannot be read leaves the list); the
tiles are drawn dimmed (`Thumb.offline`, opacity 0.4 on the picture)
and badged from their rows; the filter works over them. Their pictures
come from the cache alone: a `Job::CachedThumbnail` carries the row's
hash and stamp, looks the entry up on a rayon thread and delivers the
picture or a `NoThumbnail`, which for an offline frame marks the cell
filled and not "Unreadable" — the dimming says it. When the root comes
back (the next read finds it), the frames whose picture the cache did
not have are asked for again.

Nothing runs over an offline frame, and nothing is written under its
root. The review's repro for the first cut: root `b` renamed away, its
`z` flagged reject, Move rejects took `z` (it was in the list now) and
`create_dir_all(b/day/rejects)` made `b/` again on the local disk, so
the next pass found the folder no longer empty and marked `z` missing;
on an fstab mount point that writes under the mount, and on a hung
share it blocks the window's thread. So:
- `to_move_out` is false for a frame under an offline root: the move
  leaves it, and makes no folder for it;
- a delete of the selection leaves such frames out and says so;
- a key, a turn, a sync, a paste, a preset or an export leaves them
  out (`load_frames` says "<root> is offline: left out");
- opening one selects it, the panel shows the default edit standing in
  (not the last frame's) with the history empty, and the status says
  "<root> is offline: nothing to develop, and no change is kept"; a
  slider moved on it is said again; a copy, a sync or a preset from it
  is refused with a word, in the loupe and in culling alike;
- the strip's thumbnail of the frame on screen follows the panel's
  geometry only for a loaded frame (the first cut turned an offline
  frame's thumbnail by the last frame's geometry).
Nothing about an offline root is marked missing, as before, and a test
now says so through the rejects' move.

**A folder's own open** goes the view's way (`roots::open_listing`, a
`Purpose::Open` over `Source::Folder` or `Source::Files`): listed off
the window's thread, its frames' rows asked of the index there (the
folder made canonical off the thread as well), each frame standing in
from its row where there is one and its sidecar read on the pool where
there is not, with §192's bar over the reads. The frame to start on is
matched to the list there too, by the folder's canonical form in hand,
so `files::select_index` and its realpath a file are gone. The view
stays what it was until the list lands: a folder that turns out empty,
or a path that is not a folder, is said and the list left alone, and a
root's chip stays lit until the folder is in. The launch takes the same
path: the state starts with no files, and the folder or the file named
lands through `open_listing` from a zero timer once the loop runs, its
frame opened from the rendering setup when the list is in before it
(§174's `select_at_start`) and directly when not. What the command line
asks of the first frame (`--preset`, `--develop-temperature`,
`--exposure`) is a `Tweak` applied to that frame's sidecar, loaded for
it, when the list lands; `--also`'s clicks go with the open either way
(`open_at_start`).

**Off the window's thread.** The rows for a view are read on a
connection of the read's own (the `Look` opens the index by its path);
the window's thread counts them for the status line, a range count.
The first cut read them on the window's thread, 24.9 ms for 3,000 rows
against 3.1 ms for the paths before, which would have been about
170 ms for the NAS's 11,711 at every report during a walk. The frames
a read brings carry their row ids, so `open_loaded` no longer asks the
index for them again (19.1 ms for 3,000, 49 ms for 12,000 on the
window's thread, gone to 1.3 and 3.1 ms). A window with a reader and no
path — the tests' — still reads the rows itself.

**Counted.** Debug builds, this branch against master built from the
same tree before the branch, each with its own library. Trees of hard
links to the sample raws, a sidecar with a rating beside every link;
the shim is a tiny LD_PRELOAD over libc (`target/scratch/shim.c`, not
checked in) that delays every file call under the tree by 2 ms and
counts them, apart the main thread's and the `.gcd`s. The review
measured the first cut on the same tree and did not reproduce my one
run's 3,688 ms for master (1,857 to 2,062 ms over its runs), so the
view's numbers are ranges over four runs each side this time.

A view of a 300-folder, 3,000-frame root, `--roots` and `--snapshot`,
2 ms a call:
- before: the 3,000 sidecars read in 1.60 to 1.61 s on the pool, the
  view in the browser 1,730 to 1,811 ms after it was asked for, on screen
  at 1,873 to 1,963 ms; 15,900 to 22,450 `.gcd` calls a run;
- after: the read (the 300 folder looks and the root's, 0 sidecars)
  648 to 654 ms, in the browser at 787 to 815 ms, on screen at
  925 to 959 ms; 5,603 to 6,103 `.gcd` calls a run, 0 to 1 of them on the
  main thread, and 2 to 4 file calls under the tree there in all (the
  review counted 7 file calls and 3 `.gcd` calls there on the first
  cut, the frame opened; the open reads off the window's thread now).
  So the view's wait over a share goes from about 1.9 s to about 0.95 s
  here, and the sidecar reads from 1.6 s to none: what is left is the
  300 folder looks at 2 ms each, which the review's runs of the first
  cut put at the same 0.65 to 0.73 s.
Local disk, one run each: before, in the browser at 276 ms and on
screen at 736 ms; after, 228 and 499 ms.

A 1,000-folder, 12,000-frame root, near the NAS's 11,711, one run
each on the first cut (not remeasured after the review):
- local: before, in the browser at 700 ms, on screen at 1,486 ms;
  after, at 639 and 1,291 ms;
- 2 ms a call: before, 12,000 sidecars read in 5.96 s on the pool, in
  the browser at 6,557 ms, on screen at 7,120 ms, 45,900 `.gcd` calls;
  after, the read 2.69 s — the 1,000 folder looks are 2 s of it — in the
  browser at 4,686 ms, on screen at 6,132 ms, 0 sidecars read for the
  view. Of the 4.7 s, 1.1 s was `open_loaded` asking 12,000 thumbnails
  one push at a time under the shim (0.45 s without it, and 0.5 s for
  master both ways); not chased.

A folder of 300 opened at launch, 2 ms a call, on the first cut
(reproduced by the review): before, 1,808 calls on the main thread
under the folder, 900 of them the sidecars, and the window up at
4.78 s; after, 306 on the main thread (the launch's listing: one
`read_dir` and a stat an entry, before the window exists, as before)
and no sidecar among them, the 300 sidecars read in 0.11 s on the
pool, the list in the browser 192 ms after it was asked for and the
window up at 1.11 s.

The launch pass over an online root still reads every sidecar under it
on the indexer's thread — §160's rule 1 hashes each to see whether the
row's meta is current, and now the XMP beside it too — off the
window's thread. The view no longer waits for it.

**Tests.** The library: a row mirrors the develop and the turns beside
the meta (rated only is not edited, a step is, a picture's default is
not, the turn and the mirror both count, a develop added is seen by
the next pass); an unreadable edit is an edit with its stars kept, and
a file that is not JSON is an edit too; a row takes the XMP beside the
frame as the editor does (alone, over an unmarked `.gcd`, not over a
marked one), and a changed XMP or a `.gcd` appearing is a change to
the row; a schema 3 library is brought up in place and its two
sidecars read again by the next pass; the schema 2 test now expects
those reads too. The window (headless): a view of the roots reads no
sidecar (by the paths read, since the tests run beside each other) and
opening a frame reads its own alone; a frame's meta follows the pass
while its sidecar stands in, a loaded frame keeps memory's, and the
badge follows with no change to the list; a key on a view frame reads
its sidecar then writes it whole, develop and all, and the row follows
the save; a turn on a view frame reads its sidecar then writes it, and
a turn on an offline frame is refused with a word and changes nothing
on disk or in memory; an offline root is listed dimmed, badged and
filtered from its rows, its frame opened says so, a key on it changes
nothing and writes nothing, and a locked folder still leaves the list;
the rejects' move and a delete leave an offline root alone (nothing
made where it was or on the drive, its rows as they were after a pass
over the other root); an offline frame opened shows the default that
stands in and is no source for a copy, a sync or a preset, in the
loupe and in culling; the culling tiles turn a stand-in by its row; an
XMP's rating shows in a list built from the rows, in the badge and the
filter, equal to what the loader gives; a folder's open reads off the
window's thread, from disk when the index does not know it and from
the rows when it does, matches the last file there, and an empty
folder is said and the list kept. The requests: an arrow onto an
unloaded frame then a key rates the pick alone once it is open, the
frame before byte for byte as it was; a preset pressed on fifty
unloaded frames then a click on a loaded one lands on the fifty; three
keys during one read all apply, in order; a click on unloaded A then
loaded B leaves B open and A's sidecar in memory with nothing said;
a read dropped with its list takes the bar down and lands on nothing; a
Purple label and no label reach an unloaded frame; an export record for
a frame that became a stand-in reaches the disk and not the stand-in;
culling with move-on and keys 4 then 2 on unread frames rates the
first 4 and the second 2 and ends on the third, and ten keys rate ten
frames; a read started under an older list fills no stand-in of the
new, and a write to the path meanwhile survives; giving up names the
dropped requests in order and lets the pick go; a frame gone before its
request ran is said and the others take the key; an unflag under the
Picks filter ends on the same frame whether the key applied at once or
waited, on the last row (the nearest shown) and in the middle (the
next); culling's undo behind a queued key takes that key back and not
the one before it, and redo brings it back.
The bar's tests moved to a folder's open, which is what still reads
sidecars; the folder-over-a-view tests land their reads first, since a
folder's open is a read now.

**Where speed was chosen over simplicity.** The stand-in lives in
`st.sidecars` beside the loaded ones rather than as an `Option`: 247
uses of `st.sidecars`, most in tests, would have changed, and the
compiler would have checked each. Instead one `from_row` vector says
which is which, and the rule above says who must load. The cost is
that a use that forgets to load reads a default edit rather than
failing to compile — the review found three such uses in the first
cut — and the mitigation is the list above, the `writable` test on
every write, and the refusals on every source. The `turns` column is a
second such choice: a stand-in could have been told to load when its
thumbnail arrives, one read a frame on the pool, but that would be the
whole library's sidecars again by another door. The request's action
is a boxed closure kept in the window's state rather than an enum of
what can be asked: seven kinds of action would be seven arms in one
place, but each caller already had its body, and a closure lets it
keep it; the cost is that nothing but the doc says what a request may
do.

**Landed on top of §198 to §202.** Master moved twice while this was
reviewed: the export lines in History (§198), the CA fix (§199), the
drawn crop (§200), and the strip and grid with cells for the screen
alone (§202). The merge was by hand. `Thumb.offline` rides on the row
as the badges do, so a cell dims through the same `set_row_data` every
other change reaches it by, and a `CachedThumbnail` is delivered as an
`Outcome::Thumbnail` through `take_thumbnail`, so it lands on its row
as every other picture does, or waits in `thumb_base` for a cell. §202
took `rows_shown` and the near-screen fill out of `rebuild_browser`,
which made the merge there a matter of one field. `--develop-temperature`
and `--exposure` go through master's `Overrides` now (laid over the
frame's edit on opening, never recorded unmoved) rather than into the
sidecar: the launch's `Tweak` names the frame for them once the list
has landed and said which frame it opens, and keeps `--preset` as its
own work. §206's Recently opened recorded the launch's folder in
`startup` because master made that list before the window ran; here
the launch's folder lands through `open_loaded` like any other, so it
is recorded there, and not while the roots' view asked for at launch
is about to take its place. Its tests land the folder's read.

**Lesser, from the second review.** The grid's want-bigger re-asks
(`browser.rs`'s range report, `take_thumbnail`) go through
`rows::thumb_job`, so an offline frame's is a cache lookup and not a
pool thread on a hung share. The launch's own folder takes its rows
from the index now: `index_path` is known when the indexer starts, not
only when it says the library is open, so the launch's listing finds
the rows; a file not there yet is a read that fails and the sidecars
read instead. An export record for a frame the list holds as a stand-in
(the list replaced while the export ran) takes the "let go" path and
reaches the disk. `tags::key_of` is gone with the re-invoke.

**What the last review changed.** Four holes, each a place where
what was asked for and what ran came apart. An offline frame's picture
was looked up in the cache at the size the grid asks for (170) while
the pool keeps pictures at the size it makes them (176,
`grid::made_size`), so the cache never answered; the lookup uses the
pool's size now, and a test finds a kept picture with the file gone.
A slider moved while a pick's sidecar was still being read was
recorded on the stand-in, refused on disk without a word, and lost
when the read landed and replaced the sidecar whole; `save_edit` now
refuses any change over a frame that stands in and the status line
says why (there is no one switch that greys the whole panel, so the
panel itself stays live and its change is refused). An export queued
behind a read took its frame and its edit from whatever was on screen
when the read landed; it now carries the path it was pressed on, and a
frame no longer on screen goes out under its own sidecar's edit as a
set of one, since the worker's open picture is another frame's. And a
read that failed, or that found no `.gcd` where the row says there is
a develop, was taken as the default edit and counted as read, so the
next key wrote the default over the frame's own; now the frame keeps
standing in (`FromRow::unread` says why), nothing is written to it,
and the next request reads it again. The re-review found the same
hole from the other side: a frame could become loaded with the
stand-in's panel still on screen (a stalled read landing after the
give-up, a key's read after it, a preset's load), and the next slider
wrote that panel over the frame's edit. So the state names the frame
whose panel is the stand-in's (`panel_stand_in`), nothing of the panel
is recorded for it (`rows::panel_is_frames`, in `save_edit`,
`record_panel`, a sync's source, a paste and a preset), and `take`
opens the frame on its own sidecar the moment that sidecar comes in
while it is on screen. Two lesser: a pick whose read a
view's read had made stale (the generation moved) opened on the
stand-in as if offline, and now asks for its sidecar again; and
`refresh_ids` went back to reading ids alone for the list, rows only
for a folder's stand-ins (45 ms against 21 ms on 12,000 rows at every
indexer word, on the window's thread).

**Not done.**
- A `.gcd` missing where the row says there is one with no develop
  (rated only) is still taken as the default: the next key writes a
  fresh sidecar without the stars the row had. No develop is lost; a
  row that says edited keeps the frame standing in instead.
- An offline frame's cache lookup is a `rayon::spawn` each, in no
  order, all on the cache's one lock: a large offline root asks
  thousands at once, the on-screen ones not first.
- The launch pass over an online root still reads every sidecar and
  XMP under it (rule 1), on the indexer's thread: on a share that is
  N round trips in the background at every launch, and every ten
  minutes with §188's timer. A pass that compared the files' mtimes
  first and hashed only when they moved would read almost none; §160
  chose the hash because a save within one tick leaves the mtime, and
  the editor's own saves call `index_file` anyway. For the roadmap.
- A view's read still looks at each folder once (§187), 1,000 round
  trips for 1,000 folders, which is now most of a view's wait over a
  slow share. The folders could be taken as readable until a pass says
  otherwise.
- `cull_select` reached by a step that is not an open (a merge, a
  refilter, culling's undo) still reads a stand-in's sidecar on the
  window's thread, one small read; an arrow or a click goes through
  `open_row`'s request. `load_frames` under `set_meta`, `turn_frames`,
  `lay_over_targets` and `set_frames` is the same safety net for a
  caller without a request (culling's undo, the menu's Copy).
- Keys queued on a pick are each a save when they run, not one save
  for all of them.
- A request's action runs when its read is in and every request before
  it has run; a slow read at the front holds the rest, which is the
  order the user asked for and what the bar is for.
- The launch's folder is listed on the main thread before the window
  exists (a `read_dir` and a stat an entry), so "no RAW files at" can
  still end the run early; only its sidecars moved.
- An offline frame's picture comes from the cache or not at all; there
  is no smaller stand-in (the row has no preview). Opening one keeps
  the last frame's picture on the loupe under the default edit.
- A schema 3 row says "not edited" until its folder's next pass.
- `open_loaded` asks thumbnails one push at a time, a duplicate check
  each (§174 noted the quadratic scan); 0.45 s for 12,000 frames, and
  twice that under the shim for a reason not found.
- The 12,000-frame and 300-frame-folder numbers are the first cut's,
  one run each; the review reproduced the folder's.
