# 86. Sidecars in the file manager: a MIME type, not a move (2026-09-19)

The roadmap's complaint: a folder of raws in the file manager is
half `.gcd` files, interleaved with the pictures when sorted by name.
Greycard's own filmstrip filters to raw extensions (§16), so the
nuisance is Nautilus, Dolphin and `ls`, doubled in the test folder
where RapidRAW's `.rrdata` sit beside ours.

**Where the sidecar lives stays decided.** `IMG.CR3.gcd` beside the
raw is the convention Lightroom, darktable, RawTherapee and RapidRAW
share, and it is what makes §72's "directories are the truth" hold:
raw and edit share a stem and a folder, so any move, copy, rsync or
sync done with the dumbest tooling keeps them together. The two
alternatives each break that. A dotfile turns a visible nuisance into
a silent loss: the hidden sidecar is the one left behind when raws
are dragged elsewhere, the one a backup exclude or a sync client
skips, and Windows does not hide it anyway. A per-folder subfolder
(Capture One's model, which works only because Capture One owns the
folder) means selecting twenty raws and copying them loses their
edits every time, and gives every reader, the library index included,
two places to look. A preference between the layouts is worse than
either, since it makes both permanent.

**Fix the presentation instead.** Register the sidecar with the
desktop: a shared-mime-info XML declaring
`application/x-greycard-edit` for the `*.gcd` glob, an icon for it,
and a `.desktop` entry that lists the type, installed under
`share/mime/packages`, `share/icons` and `share/applications`. Sorted
by type, or in icon view, the sidecars then collapse into a block of
small edit icons instead of anonymous documents between the raws, and
nothing on disk changes. It is also the groundwork for opening a raw
from its sidecar: the desktop entry's `Exec` takes the `.gcd`, and
`Sidecar::path_for` runs backwards by stripping the suffix. XMP
interop (§72) is a later, separate question: the meta section may ride
in an `.xmp` other tools already show, but the edit stays in the
`.gcd`, since darktable and Lightroom rewrite XMPs they touch.
