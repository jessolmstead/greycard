# §658. Author and Copyright in the window, and None means none (2026-11-12)

**Added.** Two fields at the foot of Settings, Editing, under EXPORT
after Edit in: Author and Copyright, a 68 px label and a line field
each, no hint. They read and write §652's `author` and `copyright`
keys.

**How they are kept.** As the sheet's other text fields are: taken on
Enter, or as the sheet closes (Done, Escape, a click outside), and a
pair that did not change is ignored. The text is kept with its ends
trimmed; empty is none, and the key is left out of the file. Several
authors are one string, split on `;` at the write as §652 has it.

**Picked up at once.** The window held the pair read at the start
(`Window::rights`). It now holds the two strings as the fields last
took them, and `start_set` makes the rights from them for each set,
so the next export carries a change without a restart. A hand edit of
`settings.json` while the window runs still waits for a restart, as
every other setting the window holds does.

**None writes nothing.** §652 wrote a said author and copyright alone
under Metadata None. We reversed that: None is the user saying no
metadata, and a hidden exception to it surprised. Under None the
writer carries no EXIF and no XMP from the rights, in every format,
from the window and from `greycard export`. The command line's
`--author` and `--copyright` lose to None too, since None is the
explicit choice; the run warns on stderr that they were not written,
rather than failing. All and No edit are unchanged.

This reverses §652's Privacy paragraph: the author and the copyright
are no longer written alone under None.

**Left.** A `©` or the year offered in the field, and a line on the
export sheet naming the pair. "Edit in" another program takes the
sheet's defaults, so its TIFF carries all metadata whatever the sheet
says (as before). The sheet does not hold the focus: Tab walks out of
it onto the panel behind, where the arrows then move a slider; the
two fields are two more stops on a walk that was already open. Both
are on the roadmap.
