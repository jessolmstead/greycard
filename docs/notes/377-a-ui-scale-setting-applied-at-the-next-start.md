# §377. A UI scale setting, applied at the next start (2026-10-19)

Several people asked for the interface larger, and a tester on a dense
screen could not work at ours. We add one setting, the Settings sheet's
**Scale** row under a new INTERFACE heading at the top of the sheet:
System, 125%, 150%, 175%, or 200%. The type and controls are scaled by
Slint's scale factor over the whole window rather than by a per-font
change. The percentage is of the system's own scale, so 125% on a Mac's
panel at 2 is 2.5, larger than the default there, and System is what
every earlier build drew. It applies at the next start, and the row says
"Applies at the next start." while the choice differs from the one the
window started at. We chose five fixed steps over a free value: the
setting applies at a restart, and a tester wants a size they can name.
The live change stays a separate item.

## Slint's variable replaces the system's scale

Slint 1.18's winit backend reads `SLINT_SCALE_FACTOR` as it creates the
window (`ensure_window` in `winitwindowadapter.rs`). The factor it names
converts the window's logical sizes in its attributes and is dispatched
as the window's scale in place of `winit_window.scale_factor()`. It does
not multiply. With it set, the backend also passes over every one of
winit's `ScaleFactorChanged` events (its handling sits under `if
std::env::var("SLINT_SCALE_FACTOR").is_err()`), so a window dragged to a
monitor of another scale keeps the factor it opened at. The variable
alone could therefore offer only absolute factors, and on a panel at 2
no choice up to 200% would be larger than the default.

The system's scale is not known before the window exists. On Wayland,
winit reports a monitor's scale rounded up to a whole number, and the
fractional one reaches only a surface. What does exist after creation is
Slint's public `Window::dispatch_event` with
`WindowEvent::ScaleFactorChanged`, which sets the window's factor the way
the backend itself does at creation. We also have the custom
application handler the window fit already gives the backend: its
`window_event` sees each of winit's events before Slint does, with the
winit window and the Slint window.

So we use both. At the start, a setting over 100% sets
`SLINT_SCALE_FACTOR` to the system's scale times the percentage, with a
marker variable beside it as the no-GPU landing has. We have no way to
know the system's scale yet. Off Wayland we use the scale the window
last opened under, which the settings keep as `ui_scale_system` (1 when
none has been seen). That guess is usually right, and then the window is
created and its first frame drawn at the factor it keeps. On Wayland,
winit reports 1 until the compositor's scale reaches the surface, so a
remembered 2 would be wrong at creation every time (3, then 1.5, then 3
again, at 150%). There we start at 1 times the percentage and let the
scale-changed event that brings the real scale carry the factor: one
dispatch and no resize. Whether winit will choose Wayland is read the
way winit reads it (`WAYLAND_DISPLAY` or `WAYLAND_SOCKET` not empty),
less `SLINT_BACKEND` naming X11 and WSL, where Slint takes X11. A wrong
answer costs one correction. All of this runs on the window's thread
before the backend is selected, earlier than `adapter::probe`, and
follows its SAFETY reasoning. Setting the variable also takes the
window's scale out of the backend's hands, which is what we want, since
the handler then owns it.

At the window's first event, the handler reads winit's own scale for the
window, which Slint's variable leaves untouched. If the guess was wrong,
it dispatches the system's scale times the percentage. It then sizes the
window to the logical size the fit planned (the fitted size, or where
nothing is fitted the size asked for over the percentage, below), given
to Slint in physical pixels:
Slint hands a logical size to winit as logical, and winit converts it at
its own scale, not Slint's. The check passes over a `Resized` event,
because Slint applies that event's size after the handler returns and
would put back the stale size. On Wayland, where the first event is the
compositor's configure, the check runs at the next event, still before
the first frame. At each later `ScaleFactorChanged`, the handler
dispatches the new scale times the percentage, and winit's own resize
for the new monitor follows. The scale winit last reported is written at
the close for the next start's guess.

At 100% nothing is set: the handler only notes the system's scale, and
the backend follows the system as it always has. A `SLINT_SCALE_FACTOR`
set by hand wins over the setting and is left alone, and the row says
so. A copy of ours inherited by a greycard started from one we started
is recognized by the marker and worked out again from that greycard's
own settings. `adapter::scrub` takes both variables off the commands we
start.

The fit (`window_fit::plan`) now measures each monitor in the run's
logical pixels: its own scale times the percentage, or the factor set by
hand. The window opens no larger than the screen leaves room for at the
scale it will draw at. Where nothing is fitted (Wayland, where the
compositor places the window, or no monitor reported), the window opens
at the size asked for divided by the percentage, no smaller than the
layout's least. It then covers on the screen what it covers at 100%,
rather than that size times the scale: at 150% on a 1920x1080 output,
asking for 1500x800 at 1.5 would have made a window of 2250x1200, larger
than the screen. On Wayland that size goes to Slint in pixels at the
opening factor (1 times the percentage), not as a logical size: given
as logical, it met the Slint bug below and the window opened the
percentage too small (1000x533 pixels at 1.5 on a 1x output, its layout
cut off).

A `--snapshot` or `--screenshot` run draws at the system's scale
whatever the setting, so a capture comes out the same under any user's
configuration. A `SLINT_SCALE_FACTOR` set by hand still wins, which is
how the website's shots ask for 2.

## A Slint bug under its own variable

With `SLINT_SCALE_FACTOR` set, Slint 1.18.1's winit backend converts
logical sizes at winit's scale rather than its own once the window
exists. `resize_window` hands winit a `Size::Logical`, so
`update_window_properties` (re-applying the window item's size when the
layout's least grows after the first frame loads, say),
`adjust_window_size_to_satisfy_constraints`, and `Window::set_size` with
a logical size all resize the window to the logical size times the
system's scale. On X11 at 200% on a 1x output, the window opened at
1760x1018 and fell to 880x527 within seconds. The same happens with the
variable set by hand at the default, so it is not ours, though the
setting now makes every user who picks a scale meet it. `set_position`
and the IME cursor area have the same mistake. `ensure_window` already
converts the attributes at the overriding factor before the window
exists. The fix converts at `self.window().scale_factor()` in
`resize_window`, `set_position`, and `input_method_request`, and treats
a logical position in the attributes as the sizes there are treated. It
is 17 lines added and 3 changed, written against the 1.18.1 source and
checked to compile under two feature sets. In a throwaway build with
the patch applied, the X11 window at 200% held at 1760x1054, its grown
least of 527 at 2. The muda context menu's positions (`muda.rs`) carry
the same mistake on Windows and macOS and are not in the patch.

We carry it as femtovg is carried: a fork of Slint at v1.18.1
(github.com/jessolmstead/slint, branch `greycard-1.18.1`) with the one
commit, pinned by rev in the root `[patch.crates-io]` table, and sent
upstream. The pin covers every Slint crate in the lockfile: slint,
slint-build, slint-macros, vtable, vtable-macro, and the twelve i-slint
crates. A git crate takes its path dependencies from its own checkout,
so patching only the winit backend would have put a second
i-slint-core beside the crates.io one. fuzz/ has its own workspace and
no Slint crate, so it needs nothing. Checked first, before the fork was
online, through a `--config` override pointing the same rev at the
local clone, and then against the fork itself once the lockfile took
it: every Slint crate resolved from it with no crates.io copy left,
the whole greycard-ui suite passed, clippy was clean, and the X11 200%
window held at 1760 pixels wide where the crates.io build collapsed to
880. The pin stays until a Slint release carries the fix.

## What was checked

Under headless weston (`--width=1920 --height=1080 --xwayland`), with
the worktree's own XDG folders and a copy of a sample raw, we took
`--snapshot` captures at `--window 1500x950`, before captures were held
at 1x:

- On Wayland at 100% and 150%: 1500x800 and 2250x1200 physical pixels,
  the same logical layout at 1.5 times the pixels.
- The 1:1 zoom: with `--zoom 1 --hide-panels`, we cropped a 400x400
  square from the middle of the viewport at 100% and at 150%. The two
  crops are identical (RMSE 0), while a shift of one pixel already
  differs (RMSE 0.014). So one image pixel is still one physical pixel
  at 1.5. The viewport's existing tests cover its geometry at factors 1
  and 2.
- The fit, on X11 through Xwayland: at 100% "monitor 1920x1080 at
  1.00x, opening at 1500x940"; at 150% "monitor 1280x720 at 1.50x,
  opening at 1200x580", a window of 1800x870 physical pixels, inside the
  screen.
- A wrong guess, with `ui_scale_system` at 2 and at 0.8 on an output at
  1. On X11 the window was corrected at its first event to 1.5 and
  1800x870, the same as a run whose guess was right. On Wayland it was
  corrected to 1.5 at the size asked for. Each of these failed at first
  and led to the physical-size and skip-`Resized` rules above.
- `SLINT_SCALE_FACTOR=1.25` by hand with the setting at 150%: the window
  drew at 1.25, the fit measured at 1.25, and the sheet showed the
  by-hand line (`--sheet settings`).
- With a breakpoint on `scale::start`, the main thread was the only
  thread alive when the variable is set.
- Wayland, with the editor run without a capture flag and the
  compositor's screenshot taken (weston `--debug`). At a scale of 2 at
  100%, the window is 3000 pixels wide. At 150%, with no scale seen and
  with 2 seen, the start was 1.5 either way. The window opened at
  1000x533 logical (1500x800 pixels), and the compositor's scale of 2
  came as one event to 3, ending at 3000 pixels wide, the 100% window's
  footprint. On a 1x output at 150%, the window was 1500 pixels wide,
  1000 logical at 1.5. Both hold with and without the Slint patch. An
  earlier version that set the size as logical opened at 1000x533
  pixels on both outputs, too small, until the patch was applied.
- The Slint bug on X11 at 200% (live, a 1x output): unpatched,
  1760x1018 fell to 880x527; patched, it held at 1760x1054.
- Unit tests: the decision over the environment (by hand, ours
  inherited, removed at 100%), the factor wanted from winit's scale, the
  hold to a choice and the row's words, the plan under the scale, the
  setting's round trip through `settings.json`, and the row on the sheet
  with its next-start line, clicked through.

## What was not

- macOS and Windows. Both pre-render the first frame before mapping, as
  X11 does, so a wrong guess there shows one frame at the old factor
  before the correction. That happens after a change of screen between
  runs, not in steady use. A Retina panel's 2 is the case the guess
  exists for, and we have not seen a run there.
- A window dragged between monitors of different scales during a run.
  The compositor's first scale on Wayland, which goes through the same
  path, was exercised; a move between two outputs was not.
- That the system's scale is written at the close: a capture writes no
  settings, and a headless run never closes cleanly.
- A fractional Wayland scale: weston's headless outputs are whole
  numbers.
- Without the Slint patch, any logical size Slint applies after the
  window exists (the layout's least growing, a constraint, a resize
  from the app) still shrinks the window by the percentage on every
  platform. The opening size is held to pixels, but the patch is what
  keeps it.
- The live change, without a restart. The handler's dispatch is the
  same machinery, and that item now needs mainly the cached sizes it
  lists.
