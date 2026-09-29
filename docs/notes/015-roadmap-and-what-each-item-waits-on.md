# 15. Roadmap, and what each item waits on

Moved to `docs/roadmap.md` (2026-09-06, late): a list to glance at
and add to, one line an item, blocked items saying what they wait on.
It absorbed this section's list and our own list of what a
professional editor has to have. New blocked items go there; the
reasoning behind them stays here, by section. The rest of this section
is the editor's feedback rounds, which are record, not roadmap.

**First feedback round (2026-09-06, evening).** Four points from the
first look at the window, all taken:

- *The filmstrip goes along the bottom.* It does now, a `Flickable`
  of thumbnails under the viewport and the panel.
- *The previews are far too dark, and skin fries at +2 EV.* The
  viewport was scene-linear with a hard per-channel clip and no tone
  curve, so mid grey sat at 18 percent and a bright skin tone lost its
  red channel first. The viewport now applies a display curve in the
  shader, Narkowicz's fit of the ACES output transform per channel:
  mid grey rises about half a stop, the top rolls off, and a channel
  near its limit desaturates instead of clipping, which is what film
  did and what the eye reads as bright rather than burned. A checkbox
  turns it off for the linear view. This is a consumer's choice, kept
  in the UI crate (§5: the engine takes no tone-mapping choice), and
  the first of many: the curve is a placeholder for the editor's own,
  not a decision.
- *Exposure carried over between photos.* The panel now belongs to
  the file: each file's edit is kept on the UI side and restored when
  its thumbnail is selected, with the as-shot white balance filled in
  the first time a file opens.
- *Double-click a slider to reset it.* The standard `Slider` has no
  such thing and a `TouchArea` over it would take its drags, so the
  panel has a slider of its own: a track, a fill, a handle, sixty
  lines of Slint, with click or drag setting the value and a
  double-click restoring the default. That answers the third question
  of §5 (how painful is a custom widget): not, for a widget of this
  size.

**Second look: still two stops dark.** The curve was not the problem.
Slint's femtovg-wgpu renderer keeps every texture as plain
`Rgba8Unorm` bytes and picks a non-sRGB surface (its `wgpu.rs`, lines
77 and 331), so it composites encoded bytes unchanged; the viewport's
target was `Rgba8UnormSrgb`, which the sampler decoded to linear on
the way in, and linear shown as if encoded is a gamma dark, about two
and a half stops at mid grey. The screenshot path read the texture's
bytes directly and never saw it. The target is plain `Rgba8Unorm` now
and the shader encodes. Confirmed by eye on the second run.

Two things follow for the roadmap. Slint's renderer is not color
managed and does no blending in linear light: the viewport's output
transform (encoding, and later the monitor profile and any HDR) is
entirely the shader's, which is where §5 wanted it anyway. And
anything checked by screenshot must be checked as composited too; a
window on screen is the only test of what a window shows.

**Judged by eye (2026-09-06, late):** with the encoding fixed, the
default brightness with the curve on is about right, and the custom
slider feels good to drag. So of §5's three questions the spike has
answered the texture import (yes) and the custom widget (fine); what
remains open is the panel's look, which waits on the design pass.

**The design pass (2026-09-06, late).** `ui/theme.slint` holds the
tokens: eight surface colors stepping forward in lightness, three
text colors with contrast measured on the panel surface (12:1, 6.8:1,
3.6:1, the last for disabled labels only), an accent at 7:1 with hover,
pressed, muted and selection variants, a type scale of five sizes, a
spacing scale of six, three radii, one control height. Everything in
the window reads from it. `ui/controls.slint` is the control set:
a section header with a Lucide icon (ISC, `ui/icons/LICENSE`) and a
rule; the slider, now with hover and pressed growth, a focus ring, and
arrow-key nudging; a switch; a button; a segmented control for the
demosaic choice. Each has rest, hover, pressed, focused and disabled
drawn from the tokens. The standard widgets that remain (the scroll
view) take Slint's fluent-dark style from `build.rs`. Time: about an
hour for tokens, five controls and the re-laid panel, which is the
second question of §5 answered as far as one pass can: the cost of
making Slint look finished is the cost of writing the tokens down,
not fighting the toolkit.

**Third feedback round: white balance lags.** Temperature and tint
went through the engine, where white balance is gains on the mosaic
before the demosaic (§13 rules), so each slider step was a develop, a
second at 24 MP, and the slider read as broken. Now the viewport
previews it: the panel's white point resolves through the same
profile to gains and a camera-to-working matrix (microseconds), and
the shader applies `M_new · diag(g_new / g_base) · M_base⁻¹` in the
working space to the image on the GPU, developed at the base white
point. The develop follows 300 ms after the sliders rest, through a
single-shot timer restarted on every change (all develop-bound
changes go through it now, denoise and demosaic included), and when
it lands the base moves and the preview matrix returns to identity.
Checked by two flags that take the two paths to the same temperature
(`--preview-temperature`, `--develop-temperature`): 0.8 percent RMSE
between them against 1.8 for no preview, a mean of 0.4 of 255 over the
frame, the residual at clipped highlights where the clip saw the old
gains. By eye the crops are the same. The preview is the general
pattern for the editor: anything after the demosaic that is a matrix
or a curve on the working image previews in the shader at once, and
the engine confirms at rest.

**Judged (2026-09-06, late):** the design pass reads as a good start,
the sliders feel right to drag, and the white balance preview at a
300 ms rest is fine. The panel-look question of §5 is answered well
enough to proceed with Slint; the second spike is not needed. The
toolkit decision is made: Slint.
