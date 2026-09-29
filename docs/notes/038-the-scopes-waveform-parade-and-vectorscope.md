# 38. The scopes: waveform, parade and vectorscope (2026-09-08)

The histogram of §14 had the machinery for all of these already: the
whole frame drawn small through the viewport's own shader into a
512-wide analysis texture, binned by a compute pass, read back a frame
later, and keyed on an `EditKey` so nothing is recomputed while the
picture and the edit stand still. Everything added here is a different
binning of that same texture.

**The bins.** One buffer, `scope.wgsl` writing it, `scope.rs` the
reference that the shader is written from and the tests measure the
drawing against. The histogram is always in the first `3 * 256` bins,
whatever the panel shows, because the curve editor draws it behind the
curve; the chosen scope's own bins follow. The waveform wants a level
histogram per column per channel, `3 * 256 * 256` of them at 768 KB,
which is what the buffer is sized for; the vectorscope wants a square
of 208 by 208. Only the range the scope asks for is cleared, dispatched
and copied back, so the histogram alone still moves 3 KB a frame.

**The waveform** is levels up the picture against the image's columns
across it, the three channels over one another so a neutral frame reads
grey. **The parade** is the same bins in three panels, a channel each.
Two things had to be got right for either to read. The scale: the
flattest part of a picture piles orders of magnitude more into one cell
than a textured part does, and drawing to the largest cell leaves
everything else invisible, so the densest few cells in a thousand clip
instead and the rest of the trace has the range. And the gain: over one
another the three channels add, so at a panel's gain a neutral picture
is white everywhere; each gets about a third of the range in the
overlay and the whole of it in the parade. A picture 80 pixels tall
could not hold a trace apart either, so the waveforms take 140.

**The vectorscope** is Rec.709 Cb and Cr on a square grid, one cell to
one pixel of the picture so nothing falls between, each cell colored
by the color that chroma *is* at mid luma — the middle is grey, the
rim is saturated, and the trace paints itself. The graticule is two
rings, at a quarter and a half of chroma, the axes, and the skin-tone
line at 123 degrees from +Cb, which is where the I axis of a broadcast
vectorscope puts it and where faces land. Counts here span orders of
magnitude, since a photograph is mostly neutral and one cell in the
middle takes nearly all of it, so the brightness is a logarithm rather
than a root.

Cb/Cr rather than Oklab, though Oklab is what the mixer and the color
curves use, because a vectorscope means a particular picture to anyone
who has used one and the skin line has a defined angle on it. It reads
the encoded output, as the histogram does: what is on the screen.

In the panel the four sit under one `Segmented`, and the box goes
square for the vectorscope so its circle is round however wide the
panel is. `--scope` opens on one, for a screenshot.

### Small things kept beside them (2026-09-08)

**A settings file.** `settings.rs`: the export sheet's choices and the
scope on show, as JSON under `$XDG_CONFIG_HOME/greycard`, read at
startup and written when the window closes. By the panel's own names
for them, not the engine's enums, so the file survives a rename in
either direction and an older or newer one loses only the fields it
does not know. A screenshot or a batch export leaves it alone; nobody
wants a diagnostic run to move their defaults.

**The mixer's hue track.** The slider now carries the band's own color
in the middle and its neighbors at the ends, so which way each way
goes is visible without moving it. The accent fill is left off where a
track is tinted: a fill from the left edge says nothing on a slider
whose zero is the middle.

**A pre-push hook**, in `hooks/`, running fmt, clippy with warnings
denied, and the tests. Not `.git/hooks`, which no clone carries;
`git config core.hooksPath hooks` turns it on, and `GREYCARD_NO_HOOK=1`
pushes past it.

### The tile, and a check on every answer (2026-09-13)

The Denoiser's tile is 1024 mosaic pixels now, 1536 before, so the
network runs at 608 x 608 packed: 2 to 5 GB of VRAM for the three
shipped models against 6.4 GB and more at the old size, for 11
percent more margin to compute. And every tile's answer is checked
before it is written: any value that is not finite, or a core whose
samples sit more than 3 stabilized units from the input on average
plus a twentieth of the tile's own range, is refused with an error
that says the device may be out of memory, which the WebGPU provider
does not say itself. The bound was measured, not guessed: the noise
is unit variance in that space and the three models move a tile's
samples by 0.2 to 0.7 units on average at every noise level, the
demosaic's share scaling with the tile's range; what a failed
allocation returned on the integrated GPU measured 5.6, and a first
bound of 16 let it through. The check reads the tile it already has,
so it costs nothing. The tile chosen from the device's memory would
be better than a fixed default and waits on a runtime that can ask
the adapter; ort does not expose it.

Seen on the card the next day (RTX 5070 Ti, 16 GB, driver 615.71):
v20 at the new tile peaks 4.2 GB above idle and develops the 33 MP
Sony frame in 4.0 s; with a torch tensor holding 9 GB it still
scores exactly what it scores unstarved. One more gigabyte held and
the process dies: `free(): invalid pointer` when the shortfall is
small, a segfault inside Dawn's Vulkan fenced deleter under the
NVIDIA driver, during the download of the output, when it is large.
The old tile did the same at every level tried. So the two devices
fail differently: the integrated GPU answers with garbage, which the
check catches, and the discrete card corrupts its own heap, which
nothing in the process can catch, since it never returns. The check
stays for the first; the second is one more reason the tile should
come from the device's memory, and until it can, the default is
chosen so that a card with 6 GB beside a browser runs the widest
model.

### The learned denoiser in the editor (2026-09-14)

The network is in the editor's develop now, between `prepare` and
`finish` as the plan (§34) said, and the pieces around it:

- **The edit** says which tier, `noise.learned` as off, fast, balanced
  or best, and how much of the answer shows, `noise.learned_strength`
  from 0 to 1. With a tier on, the profiled denoise is not run; the
  network is the demosaic and the denoise both, and the panel greys
  the profiled controls to say so.
- **The registry** has the three tiers as models like any other:
  GPL-3.0-or-later, one file each, the sizes and hashes of v19, v15
  and v20 as exported, at a Hugging Face repo under the project's
  name that does not exist yet (the registry's test insists on that
  host). Until it does, the files copied into
  `~/.cache/greycard/models/<id>/` are found as fetched ones are, by
  size. The model sheet offers a tier the edit asks for and the store
  lacks, as it offers Subject's model, and a fetch that lands while
  the edit still wants it develops again.
- **The strength is a blend, not a run.** The worker keeps the
  network's answer and the plain demosaic of the same mosaic (both
  finished, so in the working space and turned), for the mosaic and
  tier they were made for, and a base is the plain picture taken that
  much of the way to the learned one. The two are one picture in one
  space, so the mix is linear in the denoise; at the ends nothing is
  copied. A drag of the slider is a 24 MP lerp, tens of ms, against
  four seconds for v20. The white balance or the tier changing runs
  the network again, since either changes what it was given. The
  memory is two working images kept, 288 MB each at 24 MP.
- **When it is not to be had**, the engine's own path stands in and
  the status line says which: the tier not in the store, or the
  network failed (the Implausible error of the day before, say). An
  export goes through the same job, so it carries the same answer.
- **The CLI** takes a tier name as well as a path: `--ai-denoise
  best` reads the store.

Checked on the Sony frame through the editor's export: the best tier
at full strength brings a flat wall's standard deviation from 19.8 to
3.5 (8-bit sRGB), the half blend to 11.1, and the half blend sits
0.7 levels from the mean of the other two over the whole frame,
which is the encode's curvature. The export took 7.8 s against 2.4
without, the network's 4 s and the second finish.

Not here yet, from the same plan: the DNG cache, so the four seconds
are paid once per frame and not once per session, and the weights
actually published.

### The panel in develop order, on tabs (2026-09-14)

The panel's sections now run in the order a photo is developed:
white balance, light, curves, the mixer, grading, noise, detail,
vignette, grain, demosaic. The exposure slider reaches 5 stops
either way, 3 before. Above them a strip of four tabs: Develop is
the list above; Crop is the geometry section, and leaving it shows
the frame whole again; Masks is the adjustments list with the
look sections beneath it once an adjustment is chosen (the same
sections, shown there instead, so nothing is written twice); Retouch
is the heal, clone and fill tools. Leaving Masks puts down a shape
being placed and returns the look sections to the global look;
leaving Retouch puts the tool down; choosing an adjustment from
anywhere (the `--show-mask` flag, say) opens Masks. `--tab` picks the
tab for a screenshot.

### The learned denoiser's answers on disk (2026-09-14)

The last piece of the §34 plan that waits on nothing: the network's
answer kept as a linear DNG, so a frame pays the four seconds once.
`greycard_ai::cache::Cache`, at `~/.cache/greycard/denoise/`, beside
the masks.

- **The key is what the network was given.** A hash of the prepared
  samples themselves, their size and pattern, the gains and ceiling
  they carry, the noise model, the model's id and file hashes, and the
  tiling. Nothing enumerates the settings: the white balance, the
  highlights mode, a hot-pixel option, all change the samples and so
  the key, and a frame copied elsewhere is the same frame. The hash is
  our own (a multiply and a rotate a word, Murmur's finalizer), since
  the standard one promises nothing across releases; 24 MP takes tens
  of milliseconds.
- **The file is a linear DNG** through `dng::write_linear_dng`, camera
  native with the gains divided out and the frame's own color tags,
  so it is also a file another editor opens. Reconstructed highlights
  reach the ceiling, above a file's white, so the writer grew a
  `headroom`: samples stored scaled by its reciprocal, and
  `BaselineExposure` telling a reader to brighten by as much. The
  reading side undoes both with the gains and ceiling of the fresh
  `prepare`, which the key guarantees are those of the write. The
  ceiling is in the key for that reason: with nothing clipped, the
  highlights mode changes the ceiling and not a sample.
- **Written beside and renamed**, so a crash mid-write leaves nothing
  that reads as an answer; a file that will not decode or is the wrong
  size is removed on the way past. A read touches the file, and after
  a write the least recently used go until the rest fit 8 GB.
- **Both consumers use it**: the worker's `run_learned`, which reports
  `Cached` and the status line says "from the cache"; and the CLI's
  `--ai-denoise <tier>`, which says where it kept the answer. A model
  given by path has no registry identity and is not cached.

Measured on the 24 MP R6 II frame, fast tier: the network 1.0 s, the
answer back from the cache 0.1 s including the hash and the LJPEG
decode; the file 48 MB. The linear TIFF from the cached answer sits
within 4 levels of 16 bits of the live one, 0.9 on average: the 16-bit
quantization through the headroom, invisible. In the editor the open
on a cached frame is the plain develop's time.

Not here: the weights published, which is a Hugging Face repo to make
under the project's name, and the model sheet's offer stands until
then.
