# §350. Keywords and a title typed in the window (2026-10-18)

The sidecar's meta has held keywords and a title since the first
XMP work (§42), the filter has had a Keyword chip and a `keyword:`
term, and the index has had a keywords table, but the setters were
reached only by the imports and the export. An outside review said
so, and two testers who come from Lightroom asked. This section puts
the two fields in the window, written the way the rating is.

## Where the fields sit

A METADATA section with a Title field, the keywords as chips, and an
Add keywords field. It is instanced three times and the instances
share the window's properties, so they cannot disagree: under
CULLING in culling mode, last on the Develop tab, and in the left
pane under the import buttons when the grid is open. The grid covers
the right panel, and the grid is where a set is chosen, so without
the left-pane copy a grid selection could not be keyworded. The
labels sit above the fields: the left pane is 240 px, and a standard
line edit beside a label ran past its edge. Chips are capped at
150 px and elide. A fold entry remembers whether the section is open.

## One write path

`meta::Change` gained `AddKeywords`, `RemoveKeyword` and `Title`
beside the rating, flag and label, so a typed word goes down the
rating's exact path: over the whole selection, a row standing in for
an unread sidecar read before it is written, the sidecar saved, the
XMP written when XMPs are on, the index row updated, the filter's
chips recounted, the hidden-by-filter message shown, and culling's
undo recording it. Words are settled as a file's are: trimmed,
empties dropped, each once whatever its case; removing a word removes
every spelling of it. `Change::is_key()` is true only for the
rating, flag and label, so Move on ignores a typed word; otherwise a
keyword would move the selection on and the next word typed would
land on another frame.

Over a selection the chips show the union, a word only some frames
carry dimmed, and a click on a dimmed word puts it on every frame.
Titles that differ read Mixed; a Mixed field left without typing
writes nothing, and an erased Mixed field writes nothing either,
since clearing every frame's title by backspacing would be a
surprise. The title is shown only from frames whose sidecars have
been read: the index stores keywords but not titles, so a frame
shown from its row would otherwise count as untitled and make the
field read Mixed by mistake.

## Whose title it is

The first build reset the Title field through a Slint change handler
on the frame's title, and the review found three ways that handler
does not fire: it runs from the event loop, only when the value
differs at re-evaluation, never inside a callback. A right-click on
an unchosen frame, a folder or collection pick, or a strip click
where both titles were empty could send the text to the new
selection or leave old text showing on the wrong frame. So the
window now records which frames the fields are showing, and a typed
title is written to those frames, not to whatever is chosen when the
field loses focus. When the selection changes under a typed field,
the text is handed to the frames it was typed for first, on the next
turn of the event loop since the state is borrowed at that moment,
and then the field is reset. A click on a frame, folder, root,
collection or tile commits the field before it changes the
selection. At quit, the field is sent before the worker and the
index stop, a frame still standing in read synchronously for it, so
a title typed just before closing is kept.

What is still dropped, never written elsewhere: a title typed for
frames that leave the list by a path that does not commit first,
Open folder, a Recent pick or an import switching folders, and a
title typed over a frame a rejects move takes away. The keys are
paths, so it can never land on the wrong frame.

## Completion

The field offers at most eight words beginning with what is typed.
The first build asked the index per keystroke; measured at 100k
keyword rows that was 45 ms on the window's thread, and about
200 ms at 500k, since a case-folded prefix cannot use the word
index. Now the vocabulary is read once, on the field's first focus
or first key, as a list of words with counts from the index merged
with the words on the frames in hand, ordered by how many frames
hold each. Words added or removed in the window adjust the list
directly; it is dropped and read again when a folder opens or an
index pass finishes. Words that arrive only through a background
save, an XMP re-read or a sync from another copy, are not offered
until the next pass. A comma or Enter ends a word, Tab takes the
pick or the first offer, Up and Down move the pick, Esc clears the
field and hands the keys back. The window's key handler is a
sibling of the panel, not an ancestor, so a focused field keeps its
typing and an unfocused one takes no keys.

The window folds words with the Meta's own rule, trim and ASCII
lowercase, so the chips, the union and the completions agree with
what the sidecar keeps apart. The filter's chips fold Unicode, so a
non-ASCII word differing only in case shows one filter chip but two
METADATA chips; the schema's rule is the one to change if that ever
matters.

## Costs

A selection's shared words were first computed as unique words times
frames times keywords, up to three times a key. It is one pass over
a map from the folded word to its spelling and count now, a word on
every frame when its count equals the number of frames, and the
completions reuse the set. `files_of` in the versions code used a
quadratic scan and uses a set.

Culling's `Tags` now carries the keywords and the title, so its undo
takes them back; the undo's "nothing changed since" guard compares
them too, and declines to undo a rating on a frame whose keywords
changed by an unrecorded path, a sync, an XMP read or a catalog
import. That errs toward not undoing and overwrites nothing.

## Tests

Meta: words go on and come off as a file holds them; a selection
shares the union and a title or none. Library: the keyword counts.
Window: a keyword on every chosen frame and off them all; a title on
every frame and Mixed when they differ; a comma ends a word and the
rest completes; the vocabulary read once and learning the words put
on and taken off; culling undoes a keyword and a title and does not
move on; the words reach the sidecar, the XMP and the index with a
real indexer; the fields take typing, Tab, Enter and Esc and give
the keys back; a title typed goes on its frame when another is
clicked, through a right-click, when the selection moves under the
field, when a folder is clicked, and when the window closes with a
stand-in row in the set; the same frames in another order keep the
field; an erased field over a Mixed set clears nothing.
