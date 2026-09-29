# 154. Splitting app.slint (2026-09-23)

`crates/greycard-ui/ui/app.slint` was 4,949 lines, one `App`
component: 855 lines of interface (307 public properties, 132
callbacks, 19 private properties, 13 functions), the key handler,
and a tree of some 3,600 lines under it. It follows `main.rs` the way
the note on splitting `main.rs` said it would, on the same rule: one
file a section of the tree, a pure move, reviewed as one.

`App` stays the one exported `Window`, and its interface does not
move: the Rust side calls 616 of the accessors Slint generates for
it, so every public property and callback keeps its line, name,
type, default and doc comment. The 439 public declaration lines are byte for byte what
they were, and `git diff master -- crates/greycard-ui/src
crates/greycard-ui/build.rs` is empty. `slint_build` follows imports,
so `build.rs` did not change either; its output lists a
`rerun-if-changed` for every new file, and touching one rebuilds the
crate. Slint's `global`s were not an option for the same reason as
renaming: the Rust side names `App`'s own accessors.

What moved where, under `ui/panel/` (the names follow `src/panel/`
where a section belongs to one of its modules, and the tree
otherwise):

- `cull.slint` the CULLING section; `mask.slint` ADJUSTMENTS;
  `curve.slint` CURVES; `retouch.slint` RETOUCH.
- `assets.slint` PRESETS, CAMERA PROFILE, LOOK.
- `color.slint` WHITE BALANCE, COLOR, COLOR MIXER, BLACK & WHITE,
  COLOR GRADING, TINT.
- `edit.slint` LIGHT, NOISE, DETAIL, SHARPEN, VIGNETTE, GRAIN,
  DEMOSAIC: the sections that are sliders on the edit and nothing
  else. `lens.slint` LENS, which is as long as the rest of them.
- `crop.slint` CROP, ROTATE, PERSPECTIVE; `display.slint` SOFT PROOF
  and MONITOR.
- `navigator.slint`, `history.slint` (SNAPSHOTS and HISTORY) from the
  left bar; `scopes.slint` the panel's pinned head (the file, what it
  was shot at, undo and redo, the scopes).
- `viewport.slint` the viewport and everything drawn over it;
  `strip.slint` the filmstrip; `grid.slint` the grid.
- `deliver.slint` the export sheet; `preset-sheet.slint`,
  `model-sheet.slint`, `rejects-sheet.slint` the other three.
- `filter.slint` `Chip`, `LabelChip` and `FilterBar`, which the grid
  and the CULLING section share; `ui/structs.slint` the six structs,
  beside `theme.slint`, since both `app.slint` and the panel files
  name them and Slint has no circular import. Rust still gets every
  struct from `include_modules!()`: the generator emits the structs
  the exported component uses wherever they are declared.

`app.slint` is 1,687 lines now: the interface (802), the key handler,
and the frame the sections sit in (the left bar's two buttons, the
tabs, the ScrollView and the Fit and Export buttons), with an
instance a section. The panel files come to 4,510 lines, and
`structs.slint` to 56.

Each moved section is a component that inherits what its root was
(`Section`, `Rectangle`, `VerticalLayout`), so the element in the
layout is the same element: a `Section`'s children go to its
`@children` slot as they did, and the `if` that decided whether it
showed stays on the instance in `App`, unchanged. The component
declares every property of `App` its body reads, under the same name,
so the body's `root.x` reads its own bound property and not a line of
it had to change: `in-out` where `App`'s is `in-out`, bound
`x <=> root.x`; `in` otherwise, bound `x: root.x`; a callback of the
same name forwarded to `App`'s. That wiring is about 1,000 of the
1,443 lines the diff adds; the rest are imports, the component and
instance lines, blank lines and the file headers.

What Slint forced beyond the move:

- **`keys.focus()`.** Twelve places in the moved sections gave the
  window's `FocusScope` back its keys. An element id does not reach
  across a component, so each became `root.focus-keys()`, a callback
  the instance forwards to `keys.focus()`. The call is still
  synchronous; nothing else in those handlers changed.
- **Image paths.** `@image-url` resolves against the file it is in,
  so the 87 icon paths in the moved sections are `../icons/`.
- **Two ids `App` reads.** `texture <=> view.source` and
  `view.width` in the interface, and `strip.content-x`,
  `strip.width` and `strip.content-width` in the strip's functions,
  named elements that are now inside components. Rather than change
  those lines, the instances carry the ids (`view := Viewport`,
  `strip := Filmstrip`) and the components alias what is read:
  `in property <image> source <=> view.source` and
  `in-out property <length> content-x <=> strip.content-x`,
  `out property <length> content-width: strip.content-width`. The
  width is the component's own, which is the Image's and the
  Flickable's since both fill it. The alias had to be `in` and not
  `in-out`: Slint warns that linking an `in` property to an `in-out`
  one is deprecated.
- **`App`'s functions the sections call** (`strip-relay`,
  `strip-report`, `grid-relay`, `grid-wheel`) stay in `App`, since
  `App`'s own handlers call them too, and reach the sections as
  forwarded callbacks. The ones only a section used went with it:
  `strip-scroll` into the strip (where `strip` is the Flickable again,
  so its body reads exactly as before), `export-edge`,
  `export-resizes`, `export-width` and `export-height` into the export
  sheet, which forwards `App`'s pure callback `custom-edge` for them.
- **Private properties.** The same rule: eight that one section used
  moved with it (`dropper` to the viewport; `spectrum` to COLOR;
  `mixer-superseded` and `mixer-acts` to the mixer, which now binds
  `bw-enabled` and `target` for them; `bw-strength-cents` and
  `bw-applied` to BLACK & WHITE; `grid-pitch` and `grid-left` to the
  grid, which forwards the pure callback `grid-slack`). The eleven
  shared ones stay and are passed in: the strip's and the grid's
  geometry, which `App`'s functions also read, `band-colors`, `dull`,
  `hue-circle` and `canvas-colors`. So `grep -c 'property\|callback'`
  over the interface block goes from 458 to 450, and the eight are
  exactly the difference; the public count is 307 and 132 before and
  after.
- **Indentation.** The moved bodies are dedented to their new depth,
  so the dimmed diff wants `--color-moved=dimmed-zebra
  --color-moved-ws=allow-indentation-change` to grey them out. The
  NAVIGATOR's body was already four spaces short in `app.slint` and
  is indented properly in its file.

No `changed` handler, `init`, `animate`, `PopupWindow` (the
ComboBoxes' own) or `forward-focus` needed anything: each moved with
the element it belongs to and runs on the same property. `changed
selected`, `changed grid-open`, `changed thumbs` and the two
`changed collapsed-…` handlers are in the interface and stayed.

**The snapshots.** 46 captures of the whole window, each flag
combination on a copy of three sample raws (two with sidecars, one
with a repair patch, one with none): the Develop tab at eleven
`--panel-scroll` positions from 0 to 5000 px, which reach DEMOSAIC
at the bottom; a file without a sidecar and one with a patch; `--zoom
1`; the Wave, Parade and Vector scopes with `--clipping`; `--proof
--gamut-warning` at the SOFT PROOF section; `--sharpen-mask`; Masks
with `--show-mask 1` at six scrolls and `--show-mask 2`, and a file
with no masks; Crop at two scrolls; Retouch with `--patch 1` and
without; `--grid` at two cell sizes and with `--filter`; `--cull`,
`--cull-compare 2` and `4`, and with `--filter`; `--ask-rejects`;
`--snapshot-placeholder`. The export, preset and model sheets and the
crop, level and guide tools in the viewport have no flag, so a
scratch patch (never committed) opened them from an environment
variable just before the capture, in a second binary built from the
same tree with the patch applied for the length of its build.

The first attempt on the desktop measured the desktop: the window
came up at 1.5× on one monitor and 1× on the other, a key typed
elsewhere reached it (one capture was in culling mode, one had the
grid open, two had been clicked to 1:1), the pointer lit whatever it
was over, and a focused window drew a LineEdit's cursor. The set runs
instead inside a headless mutter of its own (`mutter --headless
--wayland --virtual-monitor 1920x1200` on a private session bus, the
editor on its Xwayland, `SLINT_SCALE_FACTOR=1`). The Wayland client
inside a headless mutter never drew a frame, and the X11 one did.
There master's binary taken twice gives 10 captures byte-identical
and 36 identical once the status plate (x 240–1180, y 755–810) is
painted out of both, since its line carries the develop's time in
milliseconds. That is the noise floor, and nothing else varied.

The split's binary, built from the last commit and taken twice the
same way: 92 captures, 91 matching master's (21 byte-identical, 70
identical outside the status plate). The one left, the file with a
repair patch at the top of Develop, differed from both of master's
captures in two pixels at the lower end of the panel's scrollbar
handle, by one or two levels. Taken fourteen more times with each
binary, master's came out two ways at that spot and the split's
three, and in a 1280-wide window master's own binary varied the same
way at the handle's new position: the end of the handle is drawn at
a position that varies from run to run by a fraction of a pixel, on
master as on the branch. It belongs in the noise floor, which the
baseline pair had been too few to show. Nothing else differed.

**The review** rebuilt both binaries from clean trees and matched all
46 captures, filled the set's gaps (every section folded, a custom
export edge, a 30-frame folder opened late with the strip scrolled,
scale 1.5 on a 2560×1600 monitor, a 1280×720 one) with 43 more pairs
that matched the same way, and put every instance back into `App` by
script to diff the reconstruction against master with indentation
stripped: the imports, the moved declarations, the two ids and the
two forwarded pure callbacks were the whole difference. Its strongest
instrument was a differential fuzz on the testing backend: 48 seeds
of 400 pointer and key events from twelve starting states, at scale
1 and 1.5 and wired to the real callbacks, dumping every readable
property and every callback fired after each event; the two traces
were byte-identical over 140,946 lines. That covers the focus chain,
Escape and Return, and what follows a sheet's closing, which no
capture does. The generator and the reconstruction script are kept
in `tools/split/` for the next move of this kind.

**For the second pass.** `app.slint`'s interface is now most of it,
and every section repeats the declarations of what it binds: 507
declaration lines and 511 binding lines to move 3,700. That is the
price of Rust talking to one component; the way down is fewer, wider
properties (a struct for the lens's eighteen, the mixer's arrays),
which is a change to the Rust side and so not this pass's. The
`focus-keys` forward in five components is the same line five times, as
the `root.x` forwards are. `hue-circle` is declared in `App` and used
by the TINT and LENS sections only; it could live in `theme.slint`
beside the other tokens, as could `band-colors` and `dull`. And the
tab bar's handler in `App` (put the crop down away from Crop) holds a
decision that belongs to Rust with the tab change it already hears.

**Landed after, the same night.** The scratch hook became two flags:
`--sheet export|preset|fetch` and `--tool crop|level|guide`, opened
after the panel scroll and given longer to settle before the capture,
a tool bringing the Crop tab with it, the model sheet with sample text
in place of a download's. `panel::viewport::Shown` holds them with a
headless test; a snapshot of every sheet and tool no longer needs a
patched second binary.
