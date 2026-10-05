# 243. One frame in two places, part 1: the file's side (2026-10-04)

The first of §233's two parts, built in `greycard-edit` alone: state
ids, revisions on every save, a time and a revision on each field of
the meta and on the turn, and the comparison and the join as plain
functions over two `Sidecar`s. The editor's side (the queued write
behind the save, the pending list, the catch-up pass, the status line,
the frame following its archive copy) is part 2 and nothing of it is
here. A reviewer read the first cut against §233 with probes of its
own and found the comparison and the join losing data in reachable
cases; the rules below are the second cut, and where they correct
§233 they say so.

**State ids.** Every state carries `id`, the blake3 hash over its
parent state's id, the edit as compact JSON (`serde_json::to_string`)
and the step's label, the three joined by newlines, which the first
two cannot contain; the first state's parent is sixty-four zeros
(`sync::ZERO_ID`). The id is computed in `Sidecar::record_as` when the
state is recorded and stored, on `Step::id` and `Sidecar::current_id`
(written as `id` beside `step` and `exported`), and undo and redo move
it with its state. Where `take_sources` merges two equal neighbors the
merged state keeps the later state's id, the completed one, so a copy
that merged does not read as behind one that did not. Nothing
recomputes an id from a loaded file, with the one exception §233
allows: a sidecar with no ids gets them on its first read
(`Sidecar::read`, and `Sidecar::fill_ids` for whoever holds one in
memory), from the first state forward, and a save then writes them.
`record_as` fills before it records too, so a sidecar built in memory
and never read gets a chain that starts at the same zero. A state
recorded on two machines from the same parent has the same id; a
sidecar from before ids and the same states recorded by this build
come out with the same ids, which is what lets the first comparison
after the build see which copy is behind. Export records are not in
the hash.

**Revisions.** Every save (`save_in`, now `save_in_stamped` under it
with a `sync::Stamp` of the time and the host) appends a
`sync::Revision`: a hash, the time in seconds since the epoch as the
rest of the file keeps its times, and the host's name. The hash is
blake3 over the previous revision's hash (`ZERO_ID` for the first) and
the file's content, the first sixteen bytes as hex. *The content is the
sidecar as compact JSON in the order it is written, with the
`revisions` list and the `meta_at` times taken out*: the edit, the
labels, the ids, the export records, the meta, the turn, the XMP mark,
the save count, the absorbed list, the history and the snapshots. The
list cannot be in it; the times are not because they name this very
revision (below), so the hash is made first and the fields are stamped
with it after. The two are taken out of the struct and put back rather
than copied, and the JSON is serialized straight into the hasher
(`blake3::Hasher` is an `io::Write`), so a sidecar of some megabytes
is not built as a `Value` and then a buffer on the window's thread for
every save: on a 21 MB synthetic sidecar in a debug build the hash
takes 200 ms where the first cut's `to_value` then `to_vec` took 268,
against 302 for the pretty print that writes the file. A save that
changes nothing still makes a new revision, since the previous hash is
in the chain; that is wanted, a copy saved again is later than the one
it was copied from. The last two hundred are kept, oldest dropped
first. Each is written as one line, `"<hash> <seconds> <host>"`, so two
hundred of them pretty-printed are about ten kilobytes rather than a
block of objects, and the host is kept on every one rather than only
the latest: a line reads as "which machine saved this, when", which is
the kind of thing someone opening a sidecar wants to know. A line that
will not read is dropped and the rest kept; a time that will not parse
is 0, which the join treats as unknown. The sixteen-byte hash is for
the list's size; a state id keeps blake3's full thirty-two, since there
are at most fifty of them and it is the identity.

**Field times, and the revision that set each.** `meta::Times` has one
`meta::Set` per field (rating, flag, label, keywords as one field,
title, caption, and the turn): the time it was set and the hash of the
revision whose save set it, written as one line `"<seconds> <revision>"`
under `meta_at` beside the meta, not inside it, so two metas that say
the same thing are equal whatever the times, and `Meta::is_empty` and
the library's meta read are untouched. The turn is a field here (§233
did not say; the reviewer found a rotation made on the other copy
lost), so a frame turned on one machine and edited on the other comes
out turned. The save stamps them: every field that differs from the
meta and turn as the file had them when read (or as they were at the
last save) takes the save's time and the new revision's hash. The
baseline is `Sidecar::baseline` (a `meta::Facts`, the meta and the
turn), filled by `read` and by the save, skipped by serde and left out
of `Sidecar`'s equality, which is written by hand for that reason and
destructures every field so one added later has to be placed there on
purpose. A sidecar never read from a file is stamped against empty
facts, so everything it says is new. The alternative, stamping where a
field is set, would have touched every place the editor assigns a meta
field; the save knows the time and the revision anyway.

*Why the revision is on the field.* §233 has the later write of each
field winning, by its time. A clock that is wrong on one machine would
then decide which value survives. The revision does better where it
can: a field's stamping revision that the other copy carries in its
lineage (its revision list, or its absorbed list below) is a write that
copy already knew when it made its own, so the other copy's value is
the later one by causality whatever the clocks say. Only when neither
copy carries the other's stamping revision, both having set the field
since they were last the same, does the time decide; on a tie a value
over none, and then the newer copy's (`meta::join`). The test
`a_write_the_other_copy_knows_loses_whatever_the_clocks_say` has the
desk's clock behind the laptop's and the desk's rating still winning,
because the desk had the laptop's save.

*A meta from before the times.* §233 has it take "the file's latest
revision time for every field". The reviewer showed that drifting: the
latest revision moves with every save, so a copy that only edited kept
beating a rating really set later on the other copy, and a field never
set was filled too and beat a flag the other copy had set. Now
`Sidecar::pin_times`, run by `read` and by `join`, gives a time only to
fields that say something and have never been stamped, and the time it
gives is fixed: the file's first revision's, or for a copy with no
revisions (a build before §233 saved it last) the file's mtime as `read`
found it, kept on the serde-skipped `Sidecar::modified`. Pinned at the
first read, written by the next save, never moved. The mtime is also
what orders an older build's change against this build's: a rating set
by an older build at a later mtime wins over this build's earlier one,
and loses to a later one, in both argument orders
(`a_change_by_an_older_build_is_as_old_as_its_file`). A field the older
build cleared to its default is the one thing this cannot see, since a
default is never stamped; it loses to a value on the other copy, which
keeps rather than loses.

*Mixed builds: the value a stamp replaced.* Each `Set` also keeps
`was`, a short hash (blake3 of the value's JSON, eight hex characters)
of the value the field had before the save that stamped it. It is for
the copy that has no revision to say when a field was set: one a
build from before §233 wrote last, or a file whose time a copy
refreshed. Such a copy's field is pinned to its file's mtime, and the
reviewer's second pass showed the mtime saying the wrong thing in two
ways: an older build that only edited carried this build's earlier
rating across under a later mtime and beat the rating this build had
set since; and a pre-times copy whose file an external copy had
re-dated beat a real change. So, first of the rules in `meta::join`:
where one side's field has a save behind it and the other's has none
(unset, or pinned from the mtime alone), and the other side's value is
exactly the value the stamp replaced, the other side is carrying the
old value, and the stamped side wins whatever the times say. A change
by an older build to a *new* value is still a change at its file's
time and wins or loses by it. Second, a copy with no revisions gives
every field that differs from the other copy's the file's mtime as its
time for the join (`Times::as_of`, made for the join and not written);
a field that says nothing takes it only where the other copy's field
has a save behind it, since a cleared field is never pinned and would
otherwise lose for being empty: an older build clearing a rating that
this build had set over some earlier value is a change at the file's
time, and wins when later. Between two copies that both lack revisions
a value survives over none, and the time decides only between two
values: every sidecar today is from before revisions, so the first
sync after this lands is that case for every frame, and a rating must
not go because the other copy got an unrelated later edit. The one
thing that can come back there is a rating an older build cleared
against another copy from before that still has it, which is the
lossless side of the choice. Then the revision rule, then the time.
The one limit the files leave against this build's stamp, in its
common shape: an older build clearing a rating or a flag that this
build set from none is undone by the join, because the files cannot
tell that clear from a copy carrying the old value (both show the
default, with no save behind it, where the stamp says the default was
replaced). The other way about, a rating from before the upgrade
cleared by an older build against a copy this build has saved keeps
the rating: value over none. Both go away once every copy is on this
build; older builds are 0.3.x and earlier.

*A join re-saved by an older build.* Such a save drops the ids, and
the ids this build refills chain in the joined order, where the
transplanted branch's states no longer hang off their original
parents; against a copy that still has the originals the pair reads
as diverged and the next join carries the branch over again as new,
unlabeled states with the same content. The reviewer's seventh pass
ran this with a real older build in the loop and found histories at
the cap with half their entries such doubles, and real edits drained
while the doubles stayed. So the cap's first phase takes any entry
whose edit stands again later in the list under the same label,
keeping the newest of each (below): a double drained under the cap
loses no content, where merging the doubles at join time could fold a
real step back and forth into one state. Re-computing ids on the
join's side is still not done, since it would duplicate the other
way. What this costs is slots while the doubles stand, never a look;
the case needs an older build in the loop and goes as copies move to
this build.

**The comparison.** `sync::compare(local, other) -> Compared`, with
`Same`, `SameEdit` (the same edit and history; the meta, snapshots and
records to join), `Behind(Side::Local | Side::Other)` and `Diverged`,
§233's five cases in order. Case 1 is the structs' equality rather
than the files' bytes, which is the same test for two files written by
one build and the right one for two that differ in nothing but
formatting; part 2 may still skip the parse when the bytes are equal.
Cases 2 to 4 are by the latest revisions, a copy's lineage being its
revision list and its absorbed list. Case 5, where either copy has no
revisions, is by state ids, computed for the comparison when a copy
has none stored and not kept.

*Case 5 corrected.* §233's rule reads "if one copy's current state and
its whole history are in the other's history in order, it is behind".
That is safe for states and nothing else: the reviewer's probe had
both copies last written by an older build, the local one only rated
and the archive's given a state, and the local read as behind, its
rating gone when the archive was copied over it; and with the local on
this build, a rating, a snapshot and a turn went the same way. The
rule now: a copy whose states all stand in order among the other's is
behind only when it also carries nothing the other lacks, the same
meta and turn, every snapshot of its among the other's, every export
record of its on a state the other has with the record. Anything else
is diverged and joined, and the join adds no state for it (below), so
the cost of saying diverged where §233 said behind is a join, never a
state.

*The undo sentence corrected.* §233 says the undo ambiguity "cannot be
told apart here and is joined, which keeps both". With one copy at
state 4 and the other at state 5, the files hold [1,2,3,4] and
[1,2,3,4,5]: the redo stack is not written, so the shorter copy's
states all stand in the longer's, and by the rule two sentences
earlier it is behind. The rule is the one to keep: the stale backup, a
copy made and the local edited on, is the common case and exactly this
shape, and §233 wants the first comparison after the build to see it
as behind rather than join every frame of every backed-up shoot.
Nothing is lost by it: the longer copy holds every state, and the
undone position is one undo away. With revisions on both, the undo and
the new state are each a revision the other does not have, the pair
reads as diverged, and the join holds every state; but the files still
cannot tell an undo from a copy that only saved again where it stood,
and that copy must add nothing (below), so the undone position is not
recorded again there either. The test covers both halves.

**The join.** `sync::join(local, other) -> Sidecar`, as §233 has it
with the corrections here. The newer copy is the one written later
(`Sidecar::written_at`: its latest revision's time, or for a copy from
before revisions its file's mtime); on a tie the greater latest
revision hash, then the greater host name, then the greater id of the
current state, so two machines joining the same pair in the same
second make the same sidecar whichever side each calls local, and once
each has saved its own the two read as the same edit. §233's "the
local copy on a tie" is gone: it made the two machines' joins differ
and diverge again. The arguments are still named local and other, for
the caller's reading; nothing turns on which is which. The states are
the newer copy's states (which of them are shared is the union
below), then the older copy's own
and its current state, then the newer copy's current recorded again
under "Reconciled with the copy on <host>", the host from the older
copy's latest revision, or "Reconciled with the other copy" when it
has none. Branch states keep their ids and their records; the two
states "recorded again" are new states with new ids and no records of
their own, so a record stays on the state it was made from. The older
copy's current state is appended as itself when it is its own, and
recorded again when it is shared but not the last shared, so the log
still says where that machine stood. *When the older copy has no
states of its own and its current is the last shared state, it had
nothing the newer copy's states do not say, and nothing is recorded
again*: the join is the newer copy's history. That is the reviewer's
finding that a copy which only saved again, or only rated, grew the
join by two states every time (and, past the revision cap, on every
retry). A state recorded again does not repeat the state before it,
as no record does, which is also how a `SameEdit` join adds no state.
Both redo stacks go.

*States that are not the older copy's own.* A build from before ids that
recorded a state at the cap drained one, and the ids this build then
fills on that copy hash a shortened chain: every state after the drain
reads as new by id while being the same edit under the same label as a
state the other copy has. The reviewer's first-sync run on a real
sidecar at the cap showed those fifty "own" states pushing the desk's
new ones out, and the passes after it found each narrower rule still
walking past or discarding a state it should not. The rule is a union,
not a line, because a joined history is not one chain: every state of
the older copy is exactly one of four things. (i) Present in the newer
copy's list by id: shared, its records merged by id, wherever it stands
in either order; a third copy meeting a saved join finds the join's own
states standing between states it has by id, and they are the join's,
not history to drop. (ii) Capped away: a state the newer copy lacks
whose successor in the older copy's order the newer has by id, or is
itself such a state, *and whose successor's id confirms it as the
parent*. Each state's id is the hash of the one before it, so a chain
the ids confirm is history the newer copy once had and dropped, by its
cap from the front or as a repeat from the middle of a run. Kept, in its
own order, as the lowest kind of state: the first cut dropped it, and
the reviewer's eighth pass found a join under the cap losing an
eleven-state branch for no reason (the newer copy's cap had taken it;
the reconciled entry after it hashed the branch's last state as its
parent, so the chain verified). One whose edit stands in the list
already does not come back at all: it is a repeat the cap took, and
would come back as a repeat the cap takes again (the reviewer's tenth
pass counted the states this rule brought back over a hundred and twenty
seeds and found every one a repeat, drained, restored and drained
again); its records go to the entry with its edit and it stays gone. The
rest come back only as far as there is room for them once the older
branch and the states this join will record again (the older copy's
current where it stood, when it is pushed; the newer copy's current
reconciled, when the last entry will not be it already) are counted, and
where there is not room for all, the earliest in the older copy's order
are the ones left out: the cap would take them again at once otherwise,
and a current recorded again for nothing would be all that was left of
the join, which is what keeps a full join the same when a copy with no
revisions meets it again. So under the cap nothing is lost for want of a
slot that was there; at the cap these go right after the repeats, before
any shared state, so they never take a slot from a state that is one
copy's own. Each run stands right before the shared state it led to, in
the older copy's order, which is where it sits in the chain the ids
verify (for history capped from the front, right after the first state):
an undo reads in time order. The join's cap takes them right after the
repeats; a record's own cap, oldest first, takes a run capped from the
front first and one from the middle of the history in its turn. Once
kept, both copies have them and they are shared states from then on, so
a copy met again neither adds them twice nor takes them back. A branch a
join transplanted does not verify, since its states hang off their
original parents, and is the older copy's own: the reviewer's sixth pass
found a join met by a third copy losing the desk's whole branch to an
unverified version of this rule. (iii) In the re-chained common prefix:
the same edit under the same label as a state already in the list,
matched by content, its records merged onto the match. Only while the
common past still stands at the first state (the drain shape) may a
match skip newer states, and then only when the state after it matches
the state after that, since one state the same by chance (two copies
recording the same look apart, which the simulation produced) is not a
chain, and a false shared state would send the newer copy's whole tail
to the cap first; after that a match counts only at the state right
after where the common past has reached, and a match at or behind it, or
none, ends the walk, so a first step back to a shared state's edit is
not merged into that state, and a coincidence with one of the newer
copy's own states does not turn that branch into shared states. (iv)
Everything else is the older copy's own branch, pushed in its own order
and never merged, whatever it looks like: a step back and forth keeps
its sequence. The branch ends at the older copy's current; when it has
no branch and its current stands where the common past reached, by id or
by content, nothing is recorded again, which is also what makes a join
compared again with the same input (the archive's write having failed)
give the join back unchanged.

*Which list states are shared.* After a join the states two copies
have by id stand in different orders, so "everything up to where the
common past reached" marked the newer copy's own states shared and
sent them to the cap first. Shared is now the set of list states the
older copy has, by id or by a content match, plus the verified
ancestors of those in the newer copy's list: states the ids confirm
led to a shared state, which the older copy once had and capped away.
The furthest point the common past reaches drives only the content
walk.

*The cap, in the join's own order.* A record's cap drops the oldest
state after the first, which in a join is whatever the list's order
puts there. The join caps its list itself, before the states are
installed. First go the repeats, of two kinds: an entry whose edit
stands again later in the list under the same label, so that the
newest of each content stays (the doubles an older build's re-save
brings back are of this kind), and an entry a join recorded again,
which its label says ("Reconciled with ..."), whose edit stands
anywhere else, since its label never matches its original's and
differs by host from one join to the next (four places over eight
rounds had put twelve such repeats in fifty-one slots, and a
reconciled repeat kept while a tablet's own states went was the
reviewer's eighth-pass case). Then the history one copy capped away
that rule (ii) brought back; then the shared states after the first,
since both copies have them; then the older branch's own states,
oldest first; and only when the newer branch is itself longer than the
cap go its oldest, after all of those. Three entries never go: the
first state, the current (which is the reconciled state when one was
added), and the entry that carries the older copy's current, by id or,
when it was matched by content, the newest with its edit. A record's
own cap (`cap_history`) takes repeats first by the same question,
`repeat_at`, before the oldest after the first, since the reviewer saw
a copy carrying doubles lose its oldest state of its own there.
§233's order of the list is kept, since it is what puts the older
copy's current one undo away; the first cut's note that "the
branches on top survive" described the record's cap, which was the
wrong one here. Taking a repeat from the middle of a run is what made
rule (ii) general rather than a rule about the front, and it needs one
more thing: a repeat's parent was whatever stood last in the list of
the join that made it, not its neighbor in a copy's order after later
joins, so the ids cannot always confirm it. A reconciled entry the
newer copy lacks whose edit is in the list already is a repeat the
newer copy's cap took, or will take first, and is not the older copy's
own either way; its export records go to the entry it repeats, since an
export is made from the current state and after a join that is the
reconciled entry. The same holds of every entry a join lets go, by the
cap or as capped-away history: its records go to the first entry with
its edit when there is one, since an export is a record of a look and
the look is still there; a record is lost only with the last of its
edit. The older copy's current is never dropped as a repeat, and the
cap never takes the entry that carries it, by id or, when it was
matched by content, the newest entry with its edit: it stays one undo
away. For that the older copy's current recorded again
carries the same words with ", where it stood" after them ("Reconciled
with the copy on <host>, where it stood"), so it reads as what it is
and a later join knows it for a repeat; and an older copy whose current
is said already, standing in the list by id or by content or with an
entry carrying its edit under its own label or a reconciled one,
records nothing twice. (In one chain the older copy's current is the
furthest its common past reaches; in joined histories the shared
states stand in different orders, which says nothing about where it
stood, so the furthest point is not used for this.)
And where the ids cannot say, the lineage can: when the older copy's
latest revision is in the newer copy's revisions or absorbed list, the
newer copy has seen the whole of it, and every state it lacks it
dropped on purpose, so nothing of the older copy's states is its own
and only its records, meta and snapshots join. The editor copies such
a pair over whole (`Behind`) and never joins it; a join met again with
its own input is this case, and comes out unchanged.

*The simulation.* Beside the shaped tests, `sync::simulation` works
three to five places and an archive at random from a seed: edits, undos,
ratings and flags, export records, snapshots, a machine offline for a
round, and a sync with the archive the way part 2 will do it (a copy
that is behind copied over, otherwise joined and the join to both), with
turns and ratings changed along the way. Its modes: a light one; a heavy
one that presses on the cap; and places on older builds with skewed
clocks, where a place now and then has its copy re-saved by a build from
before §233 (ids, revisions, field times and the absorbed list gone, a
few states recorded with no ids and that build's cap, a rating or a turn
changed) and read back by this build as a file with a time, so the
pinning, the mtime fallback and the `was` rule run, and copies with no
revisions meet copies with them. After every join it checks: that the
join is the same from either side; that joined again with either input,
once saved, it gives itself back through the lineage, and that joined
with the input's revisions and absorbed list stripped, through the union
rules alone, it is the same from either side and loses nothing of the
join by content below the cap, no record and no snapshot (not itself
again: a joined history's order cannot always confirm what was dropped
on purpose, which is what the lineage is for); that every snapshot of
either input is in it, by name and by edit; that the rating and the turn
are one input's or the other's; that an edit of either input is gone
from the join by content only when the join is at the cap; that a record
is kept while any entry with its edit stands; and that no state is lost
but by the cap, and then none of a branch while a repeat, a shared state
(but the first) or, for the newer branch, an older branch state remains,
the older copy's current always kept. A copy copied over for being
behind must have nothing the other lacks but what the cap took or an
undo put aside. At the end every place syncs until all compare as the
same, and, where the cap never came into it, every edit the archive ever
held is still in it unless it was undone. The checks after each join run
several more joins over histories near the cap, and a debug build takes
about half a second a seed, so each of the three in-tree tests runs a
dozen seeds and the ignored `many_seeds_by_hand` takes
`SIM_SEEDS=from..to` and `SIM_MODE` for more. Before this landed, a
thousand seeds ran clean by hand in the default mode, and three hundred
in each of the heavy, the older-builds-with-skew and the
older-builds-heavy-with-skew modes; after the eighth pass, three hundred
fresh seeds again in each of the four, with the two seeds that pass had
named (one a real loss under the cap, one a copy carrying doubles losing
its oldest state to a record's own cap) among them; and after the ninth
and again after the tenth, three hundred more in each on ranges no run
had used. A debug assertion in the join holds that no state it brings
back repeats an edit the list already holds. The simulation found, in
order, the repeats a joined copy carries back after the cap took them,
the record on a reconciled entry, a current that is itself a repeat, the
chance match of two unrelated edits, the record whose only twin was
pushed after the walk, and the doubles an older build's re-save brings
back, each now a rule above.

The meta and the turn join by field as above. Snapshots join by name:
the newer copy's, then the older's that are new, a same-named snapshot
with the same edit kept once and one with a different edit kept under
"<name> (on <host>)". A snapshot taken off on one copy and still on the
other comes back, since the file does not say it was taken off: a join
keeps rather than loses, and this is accepted. Export records join by
state id, each record once. The XMP mark is the newer's; `saved` is
the higher of the two, as `find` between beside and `.greycard/` wants
it. The revision lists are put together, each hash once, by time,
capped at two hundred; and both copies' latest revisions go on the
sidecar's `absorbed` list (the last sixty-four, hashes alone, about
two kilobytes, never aged by
a save), which the comparison reads as part of a copy's lineage. That
is the lineage marker for the cap: a copy left two hundred saves
behind has its latest revision long gone from the list, read as
diverged, and would be joined again on every retry; absorbed, it reads
as behind the saved join however many saves follow
(`a_copy_past_the_revision_cap_converges_after_one_join`). The join
itself makes no revision; saving it does, and that is what puts both
inputs behind it. Until it is saved it compares as the newer input
does, so part 2 saves before it compares.

**Compatibility.** Every new field is read loosely: `id` that is not a
string is none, `revisions` or `absorbed` that is not a list is empty,
a line that is not a revision or a hash is dropped, a `meta_at` field
that is not a time is never, each without costing anything else. An
older build reading a new sidecar: `Sidecar` and `Edit` are
`#[serde(default)]` structs without `deny_unknown_fields`, so `id`,
`meta_at`, `revisions` and `absorbed` on the sidecar are skipped, and
each state's `id` lands in the edit's object, where that build's
`Step` reader removes `step` and `exported` and hands the rest to
`Edit`, which ignores what it does not know, exactly as a build before
labels read a labeled history. The reviewer verified this with a real
build of master's `greycard-edit`: an old build reads and resaves the
new sidecars, dropping only `id`, `revisions` and `meta_at`, and the
ids this build then recomputes on its first read equal the stored
ones, on a real 5.4 MB sidecar too. The test
`a_build_before_ids_and_revisions_reads_a_new_sidecar_whole` reads a
new file through the old shape. After such a resave the sidecar reads
as one from before: ids refilled the same, revisions started over, the
meta's set fields pinned to the file's mtime, and a pair with it
compared by case 5. `find`, `settle` and `compare_copies` are
unchanged; `saved` stays for what it did.

One test outside the crate moved with it: the library index's
`a_same_size_save_in_the_same_tick_is_still_seen` made its same-size
change with a save, which now grows the file by a revision line every
time; it changes the digit in place instead (`replacen`, the first
only), which is the case it was written for (a build from before, or
a hand) and what the length-and-mtime check cannot see.

**The host.** `gethostname` 1.x, already in the tree through x11rb, so
no new build cost; `sync::host()` trims it, and `Stamp::now()` is the
only place the clock and the host are read. Tests make their own
`Stamp` and go through `save_in_stamped`.

**What part 2 needs, and has.**
- `Sidecar::read(&path)` reads a sidecar by its own path, migrated,
  ids filled, times pinned, the file's mtime and the baseline kept:
  the archive's copy wherever `Sidecar::find` puts it there.
  `Sidecar::load(raw)` is `find` then `read`.
- `sync::compare(&local, &other)`, then on `Behind(side)` copy the
  other file over whole, on `SameEdit` or `Diverged` `sync::join`.
- `sync::join(&local, &other)` gives the one sidecar; `save_in` it on
  the local frame (that makes the revision), then
  `Sidecar::write_to(&archive_sidecar_path)` writes the same bytes to
  the archive's copy with no second revision, so the two are the same
  file; `Sidecar::to_json()` gives those bytes for a queue that keeps
  the latest per frame. The XMP beside the archive's copy is
  regenerated from the join, as §233 says, by the editor.
- The stat check before the archive write wants the revision we last
  wrote there: `sidecar.revisions.last()` after `save_in`, its `hash`
  the thing to record per frame and archive in the index beside the
  file's size and mtime.
- A copy's host is `revisions.last().host`; the join's label already
  names it.

Tested in `crates/greycard-edit/src/sync.rs`: ids stable across save
and load and moved by undo and redo; the same state on two machines
has the same id; a stored id kept when the file spelled the edit with
an alias; a sidecar from before ids given them once, the same ids this
build records; an older-format sidecar loading and a bad new field
costing only itself; the old shape reading a new file; every save a
revision, the list capped; meta fields stamped against the file as
read; same, behind (by a new state, an undo, a rating), same edit,
diverged and the join keeping both; the newer copy by time and a tie
broken the same way from either side; the undo ambiguity, diverged
with revisions and behind without; a pre-revision pair; a pre-id pair;
the cap leaving the first state shared; the meta by field; a meta from
before the times pinned to its file; a prefix copy with a rating, a
snapshot, a turn or an export record of its own not behind; a meta
from before the times not drifting; an older build's change ordered
by its file; a turn on the other copy surviving; a write the other
copy knows losing whatever the clocks say; a copy that only saved
again adding no states; a copy past the revision cap converging after
one join; two machines joining in the same second agreeing; an older
build's copy carrying the old value not read as a change (and its real
change to a new value ordered by its file); a clear by an older build
a change unless it restores the value the stamp replaced; a hub joined
with six copies keeping each behind past the revision cap; a capped
history edited by an older build losing none of the desk's states or
its record; the cap draining the shared states and the older branch
before the newer; the older branch's own states never merged by
content (a step back to a shared edit with a record, a step back and
forth); a saved join compared again with its no-revision input giving
the join back unchanged, in the plain and the re-chained shapes; a
joined history met by a third copy keeping the states between its
shared ones and their record; a first step back to a shared edit the
older copy's own, with twenty-seven and twenty-nine desk states; a
coincidence with a newer own state not moving the fork; a snapshot
name on both; export records by id; a revision line and a time line
round-tripping; and in `lib.rs`, a merged state keeping the later id.
