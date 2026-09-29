# 192. A bar while a view's sidecars are read (2026-09-27)

Opening a root on the NAS, 11,711 frames, showed nothing for six and a
half seconds and then the grid. The window's thread was not held (§187,
§188): the list came from the index in 3 ms, and the wait was the
sidecars, read on the pool over CIFS ("their sidecars read in 6.56 s on
the pool"). Nothing on screen said so, and it read as a hang. The real
fix, the index as the sidecars' cache, is a later item; this says the
wait is work.

**What it shows.** A thin bar filled to the share of the sidecars read,
and under it "Reading 11,711 sidecars… 3,400". The count is known up
front: the index's list is taken before the read goes out, and the read
sets the total again to the files it kept (a root offline, a folder
that cannot be read, left out) before it starts on the sidecars, so the
bar can reach its end. The pool bumps a shared count (`Progress`, two
`AtomicUsize`s behind an `Arc` on the `Look`) once a sidecar;
`load_sidecars_parallel` takes it as an optional counter. A merge's read
carries none.

**When.** The window looks at the count on a `slint::Timer` every
100 ms while `loading` is set, and shows the bar only once the read has
been out 200 ms (`LOADING_SHOWN_AFTER`), so a local root, in before
that, never flashes one. The timer stops and the bar goes in one place,
`loading_done`, which every path that ends a view's load now goes
through: the view landing (with frames or empty), a folder opened over
it (`open_files`), the index unreadable, a read that could not be sent,
and the give-up past `READ_GIVES_UP`, after which the status line's
"the library's frames did not come in" stands alone. A read given up on
that lands later brings no bar back (it is dropped by its generation
before anything else).

The give-up was `refresh_view`'s alone, and nothing calls that on a
schedule: a read that hung with no report coming would have left the
bar up and the timer ticking for good. The timer now gives up the same
way (`give_up_view`, shared with `refresh_view`) once the read is past
`READ_GIVES_UP` and its count has not moved in `LOADING_STALLED`
(10 s). Not "since the last tick": at ten ticks a second, a share slow
enough to take more than a tenth of a second a sidecar would look
stopped between two ticks while still moving. A read still moving past
60 s keeps its bar on the timer's account.

**Where.** In the grid, centered over the sheet, which still holds the
list before it until the new one lands. In the loupe, centered over the
viewport's bottom, above the status plate's row and the strip's arrow:
the strip is what the load fills, so the bar sits by it. Not in the
status plate: the plate's own bar is a develop's, indeterminate, and at
launch a develop and the load run at once; two bars in one plate, one
filling and one sweeping, read as one thing. The plate is also put away
by Tab, and the load should still be said. Both are a card on the
plate's dark ground with no touch area of its own: clicks and keys go
to what is under it. No modal.

In culling the loupe's card sits higher, above the notice (the word for
the key just pressed, 40 px tall, 56 px up from the viewport's bottom),
where it first sat on top of it. Moved rather than hidden while the
notice is on: the notice is up for a moment after every key, and a
culler stepping through frames during a six-second read would see the
bar blink out at every key. It is held there for the whole of culling,
notice or none, so it does not jump as the notice comes and goes.

**What it covers.** Any view of the roots opened by hand (a chip, All
roots) and the launch's view (`--roots`, `--all-roots`), which is
`open_view` from `Told::Opened` by `library.wanted`: the same path. Not
a folder's own open (Open folder, a double-click): it still reads its
sidecars on the window's thread (§187), so there is no count to show
and no frame drawn to show it on. That is the item after this.

**Checked.** A 200-frame root of hard links, indexed, then launched on
`--roots` with the §188 shim putting 40 ms before every file call under
it: "bar shown 689 ms after it was asked for, 32 of 200 read", the
sidecars in 1.57 s, the view in the browser at 1,685 ms. The bar came
later than 200 ms here because the window's thread was busy with the
launch's own setup (the device, the first develop) and the timer's
first ticks waited for it; a view opened by hand mid-session should tick
on time (not measured). The two placements were looked at in
whole-window snapshots with the bar held up by a throwaway build.

**Tests** (headless backend, the read queued as the roots tests do).
`loading_tick` takes its "now" as an argument: the timer passes
`Instant::now()`, and the tests pass instants they pick after the
read's start, so the 200 ms delay and the give-up run on the test's
clock and a loaded machine cannot push a tick past them. A slow view
shows no bar at 100 ms, then one at 1/3 and "Reading 3 sidecars… 1" at
400 ms; then, the start put back past the delay, the window's own timer
(on slint's mock time) moves it to 2/3; landing takes it away and stops
the timer. A view that lands within the moment shows none at 0, 100 or
199 ms, nor after. A read given up on by `refresh_view` takes the bar
with it and the status says so; the late read landing brings none
back; a folder opened over a view's read drops it. The timer keeps a
read moving a sidecar every 10 s past 60 s, and gives up on it once the
count has stood 11 s. The words group the thousands.

**Not done.**
- A folder's own open still reads its sidecars on the window's thread,
  with no bar (§187).
- A view's read that is still moving past 60 s (a library far larger
  than this one on a slow share) is still given up on by the next
  report through `refresh_view`, bar and all; only the timer looks at
  the count. `refresh_view` could ask the same of it.
- The timer's first tick at launch waits for the launch's own work.
- The card is 280 px wide and spreads over the side panels below about
  840 px of window width in the loupe; there is no minimum window size.
- The bar reads "… 0" until the roots have been looked at, up to 3 s for
  each root that does not answer (`ROOT_WAIT`), since the sidecars are
  read after.
