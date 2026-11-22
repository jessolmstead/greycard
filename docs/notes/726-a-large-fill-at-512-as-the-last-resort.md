# §726. A large fill at 512, as the last resort (2026-11-21)

A generative fill runs its window at 1024, or at 512 for a small hole.
The 1024 run needs about 7.1 GiB of the card and the 512 run about 6.3,
so a card of 8 GB with anything else on it refused every large fill
(issue 142). We compared the two sizes on fourteen holes: at 512 the
layout holds and fine texture is softer. That is worth having where
the other choice is no fill, and nowhere else.

**The rule.** A window that runs at 1024 is taken down to 512 only
when 1024 cannot be had:

- the card can never hold it, by its whole size or by what the driver
  offers of it. The size is the discrete card's own figure; an
  integrated GPU's carve-out is not a size, and with free memory
  unknown the fill runs at 1024 as before;
- or everything that gets 1024 on a busy card has been tried: the
  refusal, the card given back across the worker's other models (which
  lets go of a queue's held 512 UNet too), and the fill asked again at
  1024. In an export, the frame's own waits at 1024 and then the frame
  tried again.

Only then is it asked at 512. An earlier cut of the branch fell to 512
at the first refusal, which gave permanent 512 fills on cards of 12 and
16 GB whenever the masks' or the people's models were resident. The
order above is the fix, and the reason this section exists.

**Never more than four times down.** A window past 2048 a side is not
taken to 512; it is handled as it was, and `remove_window_at` refuses a
factor past four.

**What is said.** Nothing new. A fill refused at 1024 that will be
asked again is delivered as still coming, so the short-card notice
shows only for a fill that ends refused, and then with 512's need: on
a card that would run the fill at 512 with its memory free, "too small
for a fill this large" would send the user after a smaller fill that
fails the same way. The log says when a fill ran at 512 and why.

**The cache is not keyed by the side.** The fill is looked up before
the card is asked, so a key with the side in it would miss, run again,
and set the viewport and the export against each other. A fill made at
512 is therefore kept until its patch, seed, window, or picture
changes, even after the card frees. A mark for "made below its size",
to run it again when there is room, would be its own change.

Left: no worker test reaches a large fill first asked inside the round
asked again (the Ai-level test covers it), and the frame retried for a
refusal at 1024 logs "the card given back" though nothing was.
