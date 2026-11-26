# §755. The shelf, sized to the machine and the frame (2026-11-26)

The shelf of kept bases (§378) had a budget fixed once a session, a
quarter of the memory past 4 GB and at most 4 GB, and counted nothing
but its own bases. On a 24 GB laptop whose GPU shares the machine's
memory, with 102 MP frames, the editor rested at 13.3 GB and reached
19.5 GB whenever a base was made, with 4.6 GB in swap: the open frame
held most of it, a making added about 6 GB, and the prefetch made
neighbors beside a full shelf without asking.

**The budget is worked out at each open, and before each prefetch.**
It is the least of the old cap and what is left of two thirds of the
machine's memory after 2 GiB for the rest of the editor, what the
open frame holds at rest, and room for one base being made. When it
shrinks, the oldest bases go first. A frame the user opens is never
refused; it only lets older bases go, and the frame just left is
kept.

**What counts where.** On a GPU that shares the machine's memory
(Apple silicon, an integrated GPU, a software or virtual adapter:
anything but a discrete card) what the device holds is the machine's,
and the larger measured total is charged: 57 bytes a pixel at rest
and 45 for a making. On a discrete card only the memory side counts,
23 and 18, as before; no machine with one keeps a different number of
bases at any size. With no GPU it is 24 and 33.

**What the edit keeps.** Clarity, Texture, or Structure keeps the
picture before the sharpen as full floats on the device, 16 bytes a
pixel, and Dehaze another: 1.6 GB each at 102 MP, both held at rest
when both are on. A mask's Detail, its Sharpen, a range mask under
Detail, and a refined mask keep planes of their own. These are read
from the edit of the last develop, so a section switched on resizes
the shelf at the next idle, before any prefetch.

**The room.** The sharpen's working set (§753) is not held at rest
off a discrete card and is never in use while a prefetch makes a
base, so the room is the larger of one making and the adjustment's
own planes (the sharpen's set with the local contrast's, which run
together), not their sum. One path holds the set through a prefetch,
a settle parked by the Original key; there the prefetch adds the
set's real size to what it needs.

**The prefetch asks first.** A base to be made, with its making, has
to fit beside the open frame and the shelf. If it does not, the
oldest base that is not kept goes first; if it still does not, the
prefetch is skipped and the log says the frame's size and the room.
One worker makes bases, in turn, so two are never made at once.

**On that laptop,** by the account and by its own figures: three
bases still under a plain edit, about 11.5 GB at rest and about 16.5
at the peak of a making; one base with Clarity and Dehaze on. About
1.8 GB of the drop is §753's and about 1.2 GB the base let go before
a making. Not yet measured there.

**Machines that keep fewer.** Where the open frame and one making
already fill the allowance, nothing is kept and a step back decodes
again: 8 GB with shared memory at 45 MP and up, 16 GB at 102 MP. The
old counts overcommitted those machines. With Clarity and Dehaze both
on, 8 GB at 24 MP keeps none, short of one base by 3%.

`GREYCARD_SHELF_MB` still fixes the budget, and then nothing is
sized. The per-pixel figures were measured on a software GPU and on
the one laptop, under the default edit. Not counted: a learned model's
session and the denoiser's tiles beyond the 2 GiB, a discrete card
spilling into shared memory, and a picture (a TIFF) charged as a raw.
A base let go for a prefetch that is then abandoned is lost.
