# 262. Export names and the subfolder button (2026-10-07)

Two export bugs from a tester's list, both in 0.4.0.

## A taken name gets -1, -2

When a file of the name was already there, the export and the import
made the next one free by adding ` (2)`, ` (3)` before the extension,
and a name that already ended in such a number counted on from it. The
user wants `photo-1.jpg`, `photo-2.jpg`.

**Never read as a counter.** A dash and a number at the end of a name
is common in a camera's own names (`DSC-0042`), so counting on from it
would make `DSC-0043`, which may be another frame's real name. The
number is therefore only ever added: a clash on `DSC-0042.jpg` writes
`DSC-0042-1.jpg`, a clash on `photo-1.jpg` writes `photo-1-1.jpg`, and
an old `photo (2).jpg` is an ordinary name like any other. The count
starts at 1. One rule everywhere a name is made free: `free_name` in
`output.rs` (the window's exports, the headless `--export`, the CLI's
`--on-exists`), the set's own naming in `queue::names`, and the
import's `numbered`, its backup loop and its companions, which follow
the frame's new name (`X-1.CR3` with `X-1.CR3.xmp`). Moves, Move back,
rejects, archive copies and sidecar sync never rename on a clash, and
the preset store's `stem-2` is not a name the user sees.

**A set counts the disk.** Frames of a set that share a stem (a raw and
its camera JPEG) were told apart by the set alone, and each was then
made free against the disk as it was written, so a folder that already
held `X.jpg` got `X-1.jpg` and then `X-1-1.jpg`. Under Increment the set
now counts files already on disk as taken: `X-1.jpg` and `X-2.jpg`, and
`X-3`, `X-4` the time after. Each frame moved off a file on disk says
so in the log, "X.jpg was there already: wrote X-1.jpg", as a single
export did. Overwrite and Skip are unchanged. A JPEG exported beside
itself still goes to `X.greycard.jpg`, the older rule that keeps an
export off its own source.

**Within the limit.** A numbered export name is kept within 255 bytes
by cutting the stem on a character boundary, as the import's already
was, so a name near the limit still writes.

## The button says where a subfolder export goes

With a subfolder typed, the export goes straight into it beside each
original, with no file dialog, but the button still said "Choose
file...". It now says "Export to edited", or "Export 3 frames to
edited", with no ellipsis since nothing follows, and a line under the
field says "Saved in edited beside each original." With the field
empty the button is "Choose file..." as before.

**One condition.** The label is made from `Sheet::subfolder()`, the
function the export itself branches on to skip the dialog, so the two
cannot disagree. The label therefore shows the name as the export will
use it: case kept, but `./export//web/` shows as `export/web`. A name
the export refuses (`../out`, `/x`, `.`) disables the button, which
reads plain "Export", and the reason shows under the field as a
sentence in the warning color.

**Nothing moves while typing.** The sheet is centered, so a line that
appears with the first character, or a button that changes row, moved
the field under the cursor. The line under the field keeps its height
when empty, and the primary button has a full-width row of its own
below Cancel and Add to queue in every state; that row is also what
gives a set's label room for its subfolder (about 47 characters). A
longer name is cut at the front, "Export to …final-selects", so its end
stays readable; the line under the field wraps, by character if it
must, so a long unbroken name can never widen the sheet. As a
backstop, a panel button's label now elides at its end rather than
overflowing when the system font is wider than the estimate; a button
sized by its text keeps its width, since eliding only lowers the
minimum.

**A disabled primary looked absent.** A disabled panel button takes the
base background, but a primary kept its dark text and had no border,
so on six sheets a disabled primary nearly vanished. It now dims its
text and icon and keeps a border, as a disabled secondary does.

Open: a set's result line counts exported, skipped and failed, not
renamed; the rename is in the log only, as before. A count beside
"skipped" would be a small follow-up.
