# §715. Slint 1.19, and both forks gone (2026-11-18)

**Decided.** We move to Slint 1.19.0 from crates.io and drop the forks of
Slint (§377) and femtovg (§208): 1.19 carries slint-ui/slint#14324 and
builds on femtovg 0.28, which carries femtovg/femtovg#366. fontique moves
to 0.12 with it, so the tree holds one.

**The two fixes hold without the forks.** In a headless weston on X11 at
200%, the window held at 1760x1054 by hand, under the setting, and after a
wrong guess was corrected, as §377's patched build did. On Wayland at an
output scale of 2 and 150%, it held at 3000 px. Counting wgpu's own
pipeline creations over the frame menu, 65 key steps compiled 27
pipelines, 5 of them after the first key.

Upstream converts a logical size at `SLINT_SCALE_FACTOR` rather than at
the window's current factor, which differs from ours once we dispatch a
correction. No run showed it.

**What 1.19 broke, and the fix.** Slint 1.19 animates its scroll to an
item that takes the focus, and the wheel's scroll. The panel had told the
wheel by the view moving, so it took Slint's reveal for the wheel and let
the slider go, and Period stopped partway down the Develop tab. The
panel's ScrollPane now counts the user's own scroll inputs: the
Flickable's `flicked`, called from the input event, and the bars'
presses, drags, and wheel. A slider lets go only when such a scroll, over
the frames it animates in, takes it out of view. A section is let go the
same way.

A header test that took every digit as one width now compares a label
with the same words laid out with room to spare.

**Known limits.**
- Tried by hand on Linux only at landing. Windows (the native menus'
  positions at 150% and 200%, the build script's stack, the OS setting
  that turns animations off), macOS (the first Retina frame, the new
  scroll physics), and X11 at 200% on a real display are still to see.
- The native context menu on Windows still places itself by the system's
  scale, not ours: 1.19 carries the same code (issue 179).
