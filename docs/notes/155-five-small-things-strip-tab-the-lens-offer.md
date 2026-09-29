# 155. Five small things: strip, Tab, the lens offer, presets in the left pane, a Settings sheet (2026-09-23)

Wave A's bundle, one branch of five items that each touch the left
pane or the window's chrome, built by one agent so they could not
collide, and reviewed by another that ran every claim.

**Stripped release binaries.**

`strip = true` under a new `[profile.release]` in the workspace
manifest; there was no release profile before. Nothing reads the
symbols: the release workflow builds, runs `scripts/package.sh` and
uploads, with no symbolication or crash-reporter step, and nothing in
the tree installs a panic hook or asks for a backtrace. A panic's
message and file:line come from the panic machinery and survive the
strip; only a backtrace from a release build loses its function names,
and a debug build is where one is read. Measured on Linux, one tree
built twice with `cargo build --release -p greycard-ui -p greycard-cli`
(the commit that adds it): greycard-ui 111,834,344 to 83,032,896 bytes
(-28.8 MB, -26%), greycard 57,507,192 to 45,123,224 bytes (-12.4 MB,
-22%); `file` says "not stripped" before and "stripped" after. The
sizes move with the code (the finished bundle builds greycard-ui to
83,572,544 bytes stripped), so the manifest's comment gives the saving.
The Mac and Windows builds were not measured here; on MSVC a release
build carries no debug info to begin with, so the Windows zip should
move least.

**Tab hides everything but the photo.**

Tab, as Lightroom has it, toggles `panels-hidden` on `App`: the left
pane and the develop panel leave the layout (`if`), the strip goes to
zero height and invisible, and the Viewport's status plate invisible
(`plate-hidden`), so the viewport fills the window and a fit re-fits to
it. The side panes are taken out rather than sized to zero: at zero
width their wrapping texts grew tall and raised the window's minimum
height, so the first cut opened `--hide-panels` at 1500x1564 on a
1920x1200 screen (review). Out of the layout, a file captures at
1500x950 hidden and shown alike. The strip stays in the tree at zero
height because `App` reaches `strip.content-x` by its id; it holds no
wrapping text. Tab was free in `app.slint`'s handler. A LineEdit with
the focus takes Tab itself, as focus navigation: Tab in the filter's
field moves the focus out of it rather than hiding the panels, and a
second Tab, now reaching the window, hides them. Tab is refused under a
sheet and over the grid (the grid already has the window), and with
Ctrl, Alt, Meta or Shift held. The state is not kept in the settings.
`--hide-panels` opens that way for a capture.

**The lens profiles offered on first launch.**

Until now a user who never opened the LENS section never learned that
no lens was being corrected. When a file opens (`Outcome::Opened`)
with `LensReport::NoDatabase`, `offer_lenses_once` opens the fetch
sheet unasked, its text leading with the reason ("There are no lens
profiles on this machine, so no lens is corrected for its distortion,
color fringes or vignetting."), then the size, license and place as
before, and a note that Not now asks no more and the LENS section keeps
its button. The decision is `offer_lenses_unprompted(database,
declined, asked)`, true only for no database, never declined and not
yet asked this launch. The offer also waits, without spending the
launch's ask, while any sheet is up (the window's own `sheet-open()`,
made public for this, so the Settings sheet and any later one count;
the first cut listed four sheets and missed Settings, and an Escape
meant for Settings then answered the offer Not now for good) or a
download is running, and never happens in a batch run (a capture or an
export), which nobody is watching and whose picture a sheet would
spoil. Escape now closes the Settings sheet first, as it is drawn over
the rest. Not now on the lens sheet sets `settings.lenses_declined` at
once (the close path keeps it from the file, as it keeps the last
file); the LENS section's own button still offers the sheet whatever
was answered, without the first-launch line. `--sheet lenses` shows
the first-launch shape for a capture. Checked on a fresh
`XDG_CACHE_HOME` with no system lensfun: the log says "no lens profiles
on this machine: offering the download" 0.8 s into the run.

**Presets in the left pane.**

The PRESETS section moves out of the top of the develop panel's scroll
into the left pane, under the navigator and above the snapshots and
the history, which is where Lightroom keeps it and where the pane had
room. It is its own file now, `ui/panel/presets.slint`; `assets.slint`
keeps the camera profiles and the looks. Every `App` property and
callback name stayed, so the Rust side did not change. The rows are
the history's height (control height less 6 px) and the list scrolls
in its own box, up to 200 px as the history's is up to 260 px. The
left pane does not scroll, so fixed boxes added up past the window:
with 11 history steps and 14 presets the first cut's minimum height
was 1217 px (master 950), which does not fit a 1080p or even a 1200
px screen. `Section` gained `shrinks`: a section that sets it lets its
body be given less than its full height, down to its contents' least,
and both list boxes now ask for their list up to the cap but will go
to two rows (twice the control height) when the pane is short of room,
the rest a scroll away. Sections without it are laid out exactly as
before (least, most and preferred all the full height). Measured: the
full history and 14 presets capture at 1500x950. Like its neighbors
it is not shown in culling (it was not shown on the Cull tab before
either). The develop panel now starts at WHITE BALANCE, so a
`--panel-scroll` offset chosen for a section below it now reads
further down by the height the section took, which depends on the
number of presets (master's list had no cap): 185 px with the three
seeded ones.

**A Settings sheet.**

For what is the installation's and not a picture's, on the export
sheet's pattern: `ui/panel/settings-sheet.slint`, `src/panel/prefs.rs`
(named so, not `settings`, to keep clear of `crate::settings`). First
contents: where a sidecar is written ("Beside the raw" or "Hidden
folder", `settings.sidecars_in_folder`) and whether an XMP is written
beside it (`settings.xmp_sidecars`, which existed with no control; the
guide sent testers to edit the JSON). Each is written to the settings
file as it changes, one field at a time, so a `--sidecar-folder` or
`--xmp-sidecars` for the run does not get saved by touching the other;
the close path already kept both from the file. Flipping the place
moves nothing: `st.placement` changes and each save settles its own
frame (`save_in`, as before). The sheet counts, over the open frames
(`st.files`, a folder's or the one file opened on its own, so it says
"of the N open frames" and not "this folder"), those with a sidecar in
the other place (`Sidecar::misplaced`), and offers "Move the open
frames' sidecars", confirmed by a second button that names the count.
`Sidecar::settle` does one frame and says what it did (`Settled`):
when the copy in the other place is the one `find` reads (the newer)
it is renamed into place, the hidden folder made through
`folder_under` first (`Moved`); when it is the stale one of two, it is
removed, which is what a save would have done to it (`Dropped`).
Nothing is read or rewritten, so no edit can be lost to the move; a
failure is logged and counted and the rest go on. The report counts
the two apart: "Moved 1 sidecar beside their raws. Removed 1 older
copy left in the other place." Under `--no-sidecars` the move is off,
the sheet says why, and the callback refuses too. Opened by a
Settings... button above Report a problem at the foot of the left pane
(the export button's corner of the right panel has Fit and Export and
no room) and by Ctrl+, , the desktop convention, which nothing bound.
`--sheet settings` for a capture. Left for later: the other
installation-wide choices that live in the settings file or on panel
sections today (the monitor profile, the canvas color) could move
here; they were left where they are.

**The review.** Every one of the five had something the author's own
captures did not show. The first cut of Tab sized the side panes to
zero instead of taking them out of the layout, and their wrapping text
grew tall at zero width: the hidden window's minimum height rose from
926 to 1047 px and `--hide-panels` came up 1500x1564 on a 1200 px
screen, with the picture's bottom off it. The lens offer's busy check
listed four sheets and missed the new fifth, so Ctrl+, before the
first open landed put the offer under Settings, and the Escape meant
for Settings answered it Not now for good. The presets list's fixed
box plus the history's took the left pane past 950 px, which is how
`Section` got `shrinks`. The move's report counted a stale copy it
removed as a sidecar it moved, the sheet said "this folder" over a
list that is not always a folder, and the move ran under
`--no-sidecars`. All fixed and re-verified before landing, the
reviewer reading WM_NORMAL_HINTS off the live window to see the
minimum height come back down, and pixel-diffing the develop panel
against master at the same scroll to check that sections without
`shrinks` lay out as before (only the scrollbar thumb differs).
