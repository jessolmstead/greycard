# 133. The filter grows past the flag (2026-09-21)

§123 put a three-way Show on the browser — All, Picks, No rejects —
and it has done the job it was built for, which is the first cut of a
cull. The second cut is not a flag. It is the four-star frames, it is
the reds and the greens, it is everything nobody has flagged yet, and
it is "which of these was the harbor". §117 put all four of those
fields in the sidecar and left the filter on the roadmap. This is it.

**One value, four tests.** `greycard-ui/src/filter.rs`, beside
`cull.rs` and `grid.rs` on the shelf §132 built for exactly this, is
a `Filter` of a rating, a set of flags, a set of labels and some
words, and a frame is shown when it passes all four. Within a group
the chips are any-of and an empty group is every value rather than
none: a row with nothing ticked is a question nobody asked, and the
other reading — an empty browser until something is ticked — is a
filter that starts by hiding the folder. Between the groups it is an
and, because that is what narrowing means and what every one of these
ever built does.

The rating is the one field with two readings, so it is one enum with
two arms rather than a count and a flag beside it: `AtLeast(n)` or
`Exactly(n)`, and `AtLeast(0)` is the whole folder, which is why
there is no third arm for "any". The chip that says Any and the chip
that says nothing are the same chip. Under "exactly" that same first
chip is the unrated frames and is called 0, and the toggle beside the
row is what says which reading it is in — `Stars::chip_name` decides
the words, so the row of labels is tested rather than written twice
in `.slint`.

The words are matched against the file name and the keywords,
lowercased, every word of the query having to be somewhere: two words
narrow rather than widen, which is what a search box means
everywhere, and they may land in different places — "harbor 0002"
finds the frame whose keyword is one and whose name holds the other.
The title and the caption are not searched. Nothing in the window
edits them yet (§117's own left-out list), and a field that searches
something no one can have typed is a promise the browser cannot keep.
It is two lines in `shows_text` on the day the panel grows them.

**The counts, and the wrong rule that was tried first.** A chip's
number is how many frames carry that chip's value among the frames
the *other* groups leave: its own group's chips are set aside, and
everything else being asked is applied. So with a three-star filter
on, the flag row says how the three-star frames are flagged and the
label row says how they are labeled. It is the state of the cull,
narrowed by whatever else is being asked, and it is what a culler
came to the row to find out.

The first cut read the other way — press this chip and N frames will
be listed — which sounds like the more useful promise and is not. It
is the same number for a rating chip, since a rating chip replaces
what is there. It is a different number for a flag or a label chip,
because those *join* their group. On the 33-frame sample shoot with
the picks showing, the honest-looking promise made the row read
Unflagged 28, Pick 33, Reject 18: 28 is the picks plus the unflagged,
33 is what taking the only chip off leaves, 18 is the picks plus the
rejects. Every number correct, every number arithmetic about the
filter rather than anything about the pictures, and a row that never
adds up to anything. Under this rule the same row reads Unflagged 15,
Pick 13, Reject 5 — the shoot — and the three sum to the 33 the other
groups left, because a frame carries one flag, and one label or none.
A row that adds up is a row that reads at a glance. It is what
Lightroom's filter bar shows, and it took building the other one to
see why.

What is lost is worth naming: the row no longer says how many frames
a press will leave. That is one number rather than thirteen, and it
has a place already — the `N of M` beside the text field, which
updates the moment the chip is pressed. Press Reject with the picks
showing and 18 is what it says.

`Counts::of` does all of it in one pass over the sidecars the open
folder already holds. Each frame answers the four groups once, and
then counts towards a group's row exactly when the other three said
yes — that condition *is* the group being set aside for its own row —
landing on the chip it carries, or, in the rating row, on every chip
its stars satisfy. A thousand frames is a thousand cheap comparisons
and no index; measured, two words over three thousand sidecars is
402 µs. The roadmap's "needs no index" is not a concession, it is why
the counts can be recomputed on every star press without anyone
noticing.

The rule is written down twice on purpose. In prose on `Counts`, and
as three functions — `only_flag`, `only_label`, `only_stars`, each
"this filter with that row set to just that chip" — which exist only
under `cfg(test)` and say what the one pass is supposed to come to.
The test holds every chip of every group to them under nine different
filters, and separately holds each row to summing to what the other
groups leave. Nothing in the window calls them: filtering the folder
once a chip would be fifteen passes where one will do.

**Where it sits.** The chips take the grid header's second row —
where the three-way Show was, which was one control and is now
thirteen and a text field. The header is spelled out in tokens rather
than asked of the layout, because a header whose height settles a
frame late re-flows the sheet twice on the way in, and the second
re-flow is a folder's worth of thumbnails asked for again; the one
test that reads the sheet's height caught that in a line. The row is
clipped rather than wrapped, and below about 950 logical pixels of
window width the last label chips go off the right; the panel's
spelling is the answer at that size, and the window does not usually
live there. The CULLING section has the same chips a group to a row
with its name above it, spelled closer: the panel is 320px and the
header's row is not, and a caption beside each row would take the
width three chips need. It is the same `FilterBar` either way, on a
`stacked` flag, so the two cannot drift.

**The keys: Ctrl+F and /.** Both, and the reason is that they are
different keys for different hands. Ctrl+F is what every application
on every platform means by find, nothing in this window bound it, and
it costs nothing to honor. `/` is the one-key reach for a hand
already on the arrows and the rating keys — §131's argument for the
brackets, one field along — and it is what less, vim, and a browser's
quick find mean by search. Neither is ever typed into anything,
because once the field has the focus the field has the keys, which is
also what keeps 1 to 5 a rating outside it and a character inside it.
A query already in the field is selected when the key lands, so the
next character replaces it: the key is pressed to look for something
else far more often than to add a word to what is there.

Where the cursor goes is the panel's business and it has three
answers. The grid is open: its header's bar takes it. The grid is
shut and the Cull tab is up: the panel's bar takes it — and the
section is unfolded first, because a folded `Section` clips its body
to nothing, a field with no geometry is not where a key lands, and
the press would be swallowed in silence. Anything else: the grid
opens, since its header always has the chips. The ask itself is a
one-shot flag rather than a tick, answered by whichever bar is on
screen and put straight back to false: the grid's header is built a
moment *after* the key, so the bar has to be able to answer on `init`
as well as on the change, and a flag that stays raised would have the
next bar built for any reason at all stealing the keys.

Esc in the field clears the words *and* hands the focus back, in one
press. The other spelling — clear now, leave on the next Esc — makes
the commonest case two presses, and the case it protects (I cleared
it and want to type again) is a case where the cursor is already
there. One press undoes the whole excursion. Enter hands the focus
back and keeps the words, which is how you arrow through what you
found. A second Esc, the field now being nobody's, closes the grid as
it always did.

**What happens to the frame under you.** Exactly what a reject under
"No rejects" has always done, because it is the same situation and it
was already decided: the selection goes to the nearest frame still
shown — the one after it, else the one before — and that frame opens,
which in culling is a switch and outside it a develop, as Lightroom
has it. What is new is that a star and a label can now do it too, so
`set_meta` stopped asking what kind of key was pressed and started
asking the filter whether the frame is still shown, before and after.
That is the honest question; the old one was a shortcut that was
right only while the flag was the only thing filtered. The selection
never vanishes and it never stays on a row that is no longer there.

`row_of` versus the file index is untouched and still means what it
meant. §132's badge bug — a star written at the file's number into a
list indexed by row — is the reason the headless test checks the rows
by the name on them rather than by counting them.

**When it hides everything.** A filter can leave nothing, and a blank
sheet with a 0 on it is not an explanation. There is one sentence for
it now, in `filter::NOTHING_SHOWN`, said from all four places that
can arrive there: the folder opening, a folder chosen, a chip, and a
star that takes the last shown frame out. It used to read "show All
to see them", which named a control — All was the Show's first
option, and it is now the first *rating* chip and means any rating.
The way back is Clear, and the sentence says so.

This is also the one thing a culling key says in the status line.
§117's rule — the badge is the whole answer, and the arrow to the
next frame would write over any word before it had been read — holds
while there is still a frame on screen to carry the badge. When the
list has just gone empty there is no badge, no next frame and no
arrow to write over it.

**Per session.** `Settings` does not keep the three-way Show and does
not keep this either. A filter is a thing you are doing right now,
not a preference, and a folder that opens hiding most of itself
because of something done on Tuesday is a support question. `--filter`
still takes All, Picks and "No rejects", parsed into the same value a
chip builds, so a script written against §123 means what it meant.
One thing did become a preference: the CULLING section now has a
fold, like every other section, and folds are remembered — it needed
one because a key has to be able to open it.

**Left out.** Camera, lens and ISO, which the roadmap line asked for
"if the EXIF the browser reads is on hand for the whole folder". It
is not: the shot's settings under the file name come from
`Outcome::Opened`, which is one frame, the one that is open. Reading
every raw's EXIF when a folder opens is the metadata probe §132
measured at 3 to 40 ms a file — twenty seconds on a wedding, on the
way in, for three chip rows. When there is a folder index or a cached
probe those are three more rows and `Filter` gains three more sets;
nothing else in this changes. Also left out: saving a filter as a
named set, a "not this" chip, and a range of ratings, none of which
the second cut of a cull has asked for yet. And the browser still
walks the folder twice on a rebuild, once to list and once to count;
the second walk is the one that lowercases, so halving it would save
a few hundred microseconds a keystroke, and folding the list into
`Counts` for that was not worth the shape it would have left.

Tests: `filter.rs` carries the questions each chip asks and the and
between them, the text over names and keywords, the rule that a chip counts what carries it
among the rest, over every chip of every group under nine filters,
and each row summing to what the other groups leave, the
three old names, a chip pressed twice, the chip labels under both
readings, and the filter in words. Through the window, headlessly:
the rows a filter leaves and the counts it puts on the chips, driven
through the window's own callbacks; a star that hides the shown frame
and where the selection lands, including the last frame shown, where
there is nothing after it; that Ctrl+F and / both reach the field
while a rating key and a sheet do not; and, through the real handler,
that the key opens the grid from a develop tab and unfolds a folded
CULLING section instead — proved in both by a character typed
afterwards landing in the field rather than rating the frame.
