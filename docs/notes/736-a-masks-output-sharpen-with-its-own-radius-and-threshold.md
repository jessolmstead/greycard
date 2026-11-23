# §736. A mask's Output sharpen with its own Radius and Threshold (2026-11-22)

At +100 a mask's Output sharpen did almost nothing on a slightly soft
subject (0.5 to 1 level), because it borrowed the picture's Radius 1
and Threshold 1%. At Radius 3 and Threshold 0 it moved 5.8 levels, but
only with the picture's Radius set and its Amount at 0. A mask's
Detail now has its own Radius and Threshold for the Output sharpen,
and the share reaches +200.

**The edit.**

- `Detail::output_sharpen_radius` and `output_sharpen_threshold`,
  `Option<f32>`, with the picture's ranges; none means the picture's.
  Left out of the file at none.
- The share runs −1 to +2.
- No write floor: the fields land before schema 10's cut (§734).
- No process bump: absent fields render the same to the bit, and the
  render gate (§733) passed unblessed, then gained two fixtures.
- `ids-at-10.json` gained a row.
- A share above +1 in an existing file rendered at +1 in earlier
  builds and now renders at its value, held at +2. Only a hand-edited
  file could hold one; no shipped preset does.

**The composition.** With `w` the weight times opacity and `s` the
share:

    A₀ = clamp(base·Π(1 + w·s⁻) + Σ₀ w·s⁺, 0, 2)
    Aₖ = clamp(Σₖ w·s⁺, 0, 2)
    S  = A₀ + Σₖ Aₖ;  where S > 2, every Aᵢ ← Aᵢ·2/S
    δ  = A₀·soft(l − B(r₀), t₀) + Σₖ Aₖ·soft(l − B(rₖ), tₖ)

- Term 0 is the picture's: its Amount, every take, and the additions
  at its settings. Term k is an own (radius, threshold) setting.
- `δ` moves the encoded luminance as §663's single term does. Under
  AgX's rail it is held by the sum of the amounts.
- The sum is held at 200%, every term scaled alike. A radius moved
  just off the picture's, or a setting past the cap sharpening at the
  picture's, then changes the radius and not how much a pixel takes.
  With no own term this is §730's clamp, to the bit.
- A take with settings of its own takes from the picture's sharpening
  at the picture's settings; its sliders are off.
- At most three own settings, sorted so the masks' order changes no
  bit. A fourth sharpens at the picture's, and the log says which mask,
  once each time that changes.
- A mask that acts nowhere (its shapes off, or wholly off the frame)
  still takes a slot.
- One blur per distinct radius.

**The GPU.** The blur target is an array with a layer per radius, each
blurred across by its own pass. A target of one size is kept once and
reused while it has layers enough; a deeper one replaces it, so an own
radius never pushes the scopes' target out. At 4K with four layers
that is about 174 MB a size. The last draw finds the picture's edge
once, blurs every layer down, and sums the held terms. Against the CPU
it is 0.50 levels worst on lavapipe at 1:1, at half, and at 2:1 and
4:1 at the window's edge; the viewport's suite passes on an RTX 5070
Ti.

**Cost.** A 45 MP export at four threads: the picture's sharpen 4.7 s,
with one mask at its own radius 5.6 s, with three 6.6 s; no extra peak
memory. The window on lavapipe goes from 28.4 ms (the picture's alone)
to 43.3 ms with one own radius and 67.4 ms with three. On an RTX
5070 Ti it is 1.0 ms a draw, 1.4 ms with one own radius, and 2.0 ms
with three.

**Out of reach (2026-11-23).** A share under zero scales the picture's
own sharpen, so it does nothing while that is off or at no Amount. The
slider's negative half is then drawn dimmed and no drag, key, or typed
number goes under zero. A share already stored stays, drawn muted, and
acts again when the Amount rises: nothing is written for the change.
