# 89. The filmstrip scrolls (2026-09-19)

The strip could show only the first screenful. It held a `Flickable`
already, so the bug read as a missing feature until the Flickable's
own rules were read. Slint refuses a wheel along an axis the
Flickable cannot move, and this one moves in x while a mouse sends
`delta_y`; worse, the row's preferred height ran a few pixels past
the strip's 148, so the Flickable happily claimed the vertical wheel
and spent it sliding the thumbnails up by those few pixels, and
nothing else ever saw the event. Pinning `viewport-height` to the
strip's height closes that door (the tiles shift by a pixel, 262 of
222 000 in a before-and-after of the bar), and a `TouchArea` wrapped
around the Flickable catches what the thumbnails and the Flickable
both reject and turns the vertical delta into a horizontal one, 60
logical pixels a notch, the same step every other scrolling thing in
the window takes. A wheel with any sideways component stays the
Flickable's, so the handler takes `delta-y` alone and rejects the
rest. Dragging needed no code at all: the Flickable takes the gesture
over after eight pixels and sends the thumbnail under the pointer an
`Exit`, so a drag that moved never selects and a press that did not
always does.

The selection is kept on screen by the least scroll that shows it,
with the strip's padding left beside it, from a `changed selected`
handler and from the strip's width. That one handler covers every
way the selection moves, since the arrows go through `step` and
`invoke_select` like a click, and the width hook is what makes a
folder open already scrolled to the remembered file of §65, however
far down it is. Two things had to be right there. Slint runs the
changed handlers before the first layout, when the strip's width is
zero, and every frame past the first then reads as off the right
edge and scrolls to the clamp; a width guard drops that pass. And
there is a second pass at an intermediate width, 574 px here before
the window settles at 1500, from which a least-scroll leaves the
frame at a place that is a function of nothing but that accident. So
the width handler works from the start of the strip rather than from
where it stands: `viewport-x` to zero, then reveal. The remembered
file at index 2 of forty needs no scroll and gets none; at index 32
it comes to rest against the right edge with its padding, and does
so whatever the window did on the way up. The cost is that a window
resize also re-reveals from the start, dropping a scroll position the
user had set; rare enough to take for the determinism, and easy to
revisit if it grates. The geometry the arithmetic needs (178 wide,
186 pitch, 12 padding) now lives in three properties the layout
reads too, rather than as literals in two places that could drift
apart.

Thumbnails were the other half. Every file's preview was queued at
startup and popped in file order; they are already the lowest
priority in the worker, so the open picture was never delayed, but
the strip filled 0, 1, 2 and so on, and the remembered frame at #400
was the last thing decoded. Measured here, a camera preview costs 55
to 183 ms, mean 117, §35's 40 to 110 with the bigger bodies included,
so five hundred raws is about a minute of file-order filling. Rather
than drop and re-request pictures, the strip reports the range it
shows on every move and the worker sorts the pending deque around
it: that range first in file order, the rest outward from its
middle, a free `order_thumbnails` with a test. Everything is still
made, only the order changes, and the repeat a moving strip sends is
dropped in the queue itself, which also forgets its ordering whenever
a new folder's jobs are pushed. Opening a folder of seventy-five on
the sixtieth frame now shows that frame's neighbors developed, not
the first five.

**The two the cold start left.** `greycard models` is the missing
half of `greycard lenses`: bare, it prints the store's path and then
every entry of the registry, the id, what it is for, what it weighs,
its license, and whether the store has it, with the denoiser tiers
naming themselves as tiers; `--fetch` takes a tier, an id or `all`,
prints the license and its URL, the source and the size before
anything is downloaded, then goes through `Store::fetch`, which
checks the published hash and leaves the license note beside the
file, with a progress line rewritten in place on stderr at a
terminal and one line a file otherwise. The registry gained a
`purpose` a listing can print, and `model(id)` and `tier_of` for the
lookups. Checked from an empty temp home: the listing says "not
fetched" for all six; `--fetch fast` pulled the 5 MB v19 in 0.7 s
and it hashes to the registry's value; and `develop --ai-denoise
fast` on the lighthouse frame then loaded it on WebGPU and ran in
4.7 s. The other half of the same complaint was what is said when a
tier is missing, which pointed at the editor's NOISE panel and now
names `greycard models --fetch <tier>` with the size and the
license. And X-Trans: `CfaPattern` has a `Display` that says `2x2
RGGB` or `6x6 (X-Trans)` rather than the 36-entry `{:?}` the six
Bayer-only sites and the learned denoiser's two shape complaints
were printing, and a mosaic that is not Bayer is now turned away at
the door, in `prepare`, from the layout alone, since preparing a
101 MP X-Trans frame only to refuse it at the demosaic costs a
gigabyte and several seconds of normalizing, cropping, measuring the
sharpen radius and reconstructing highlights for nothing; the
demosaic keeps the same sentence as the backstop for a consumer that
comes straight to it, bilinear included, since bilinear alone would
run on a 6x6 and make mush. There is no X-Trans raw here (the three
Fujifilm files are a GFX 100S II, which is Bayer; a `.RAF` is not
necessarily X-Trans) and rawler's checkout carries only digests for
its Fujifilm entries, so the `Display` and both gates are tested on
a synthetic 6x6, through `develop` for all six demosaic methods and
through `prepare` directly to pin which gate answers, and the wording
is untested on a real file.
