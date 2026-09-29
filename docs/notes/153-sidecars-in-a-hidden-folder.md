# 153. Sidecars in a hidden folder (2026-09-23)

A tester finds a `.gcd` between every pair of raws cluttering, and
asked for the edits somewhere else. The somewhere else is not a
parallel tree under the home directory and not a central store: §72's
rule is that directories are the truth, and a shoot copied to a drive
or another machine has to arrive with its edits, which a tree
elsewhere cannot give it without the library's hash index finding
them again. That is Lightroom's catalog problem in small, and the
people who dislike sidecars are usually the people it burned. A hidden
`.greycard` folder inside the shoot's own folder answers the
complaint, one entry a folder rather than one a frame, and the edits
still travel with the folder. Capture One's session subfolder is the
precedent people accept.

`greycard_edit::Placement`, `Beside` or `Folder`, and `Sidecar::path_in`
beside `path_for`. Reading is not a choice: `Sidecar::find` looks in
both places whatever the setting says and takes the newer when both
are there, beside on a tie, and `load` goes through it. Writing is:
`save_in` takes the placement, makes the folder when it is missing and
sets the hidden attribute on Windows every time, since a leading dot
hides nothing there and a copy to a new disk can drop the attribute.
The attribute is one Win32 call through `windows-sys`, the one
`unsafe` block in the crate; a failure to hide is a warning and not a
failed save. The XMPs stay beside the frame whatever the setting,
since beside is where Lightroom and darktable look.

Flipping the setting moves nothing by itself. The setting is the
installation's and the open folder is one shoot among many, so a move
on the flip would tidy this folder and no other, and a bulk move is
not a side effect a switch should have. Instead a save settles its
frame: after the write, the copy in the other place goes, since the
load took the newer of the two and what was written came from it. A
folder migrates a frame at a time as it is worked through, and one
`.gcd` a frame is an invariant after any save. The tidy-in-one-go move
for the rest is the Settings sheet's job, offered with a count when
the switch flips with a folder open, on the move-rejects sheet's
pattern; the sheet is a v0.2.0 line and the switch lives there, so
until it exists the setting is `sidecars_in_folder` in settings.json,
or `--sidecar-folder` for a run.

The rest follows the placement: the browser's listing skips the
folder as it skips any directory, a `.gcd` opened from under it names
the frame above it, move rejects finds the sidecar wherever it is and
keeps its placement under the rejects folder, the CLI's `--apply`
writes back where it found the sidecar since it has no setting.
Checked through the release editor on a scratch picture: a quarter
turn with the flag lands under the folder and nothing beside; without
the flag the next turn lands beside and the folder's copy goes; the
sidecar path under the folder opens the picture. An empty hidden
folder can be left behind after a frame settles beside, which is
harmless.

Landed the same day, the first branch the author wrote by hand with
the skeleton and the tests laid out first: the resolver's path lost
the raw's directory on the first try, and a stray folder in the crate
directory made a test pass for the wrong reason until the test's
path was fixed. Tests first held up.
