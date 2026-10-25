# §421. The remove's memory read from the machine

*2026-10-24*

The remove opened one session at a time with the decoder in tiles,
because neither ort nor wgpu says how much memory the adapter has.
On a GPU that shares the machine's memory the system says it, and on
a Mac that default costs most of the time.

## Measured

An M5 MacBook Pro, 24 GB unified, a 1024 removal on WebGPU over
Metal: one at a time, tiled, loads 16.3 s, encode 3.8 s, 24 UNet
calls 58.8 s, decode 9.5 s, 88 s in all. With all three kept open
and the decoder whole the first removal loads 17.2 s and decodes in
4.3 s; the second loads nothing, 71 s in all. The VAE's sessions
open in 0.05 s each: all 16 s of load is the UNet. All three open
worked at 24 GB with nothing else loaded, but 12 GB held is half the
machine.

## Decided

A third mode keeps the UNet open and opens the encoder and the
decoder for their part, so the 16 s is paid once and 5.1 GB is held
between removals. The policy comes from the machine when the
environment says nothing: on unified memory (an integrated GPU as
wgpu reports it, or any Apple silicon Mac), the UNet kept open and
the decoder in tiles from 16 GB, the decoder whole as well from 24
GB (the 24 GB Mac decoded whole even with all three open), all three
kept open from 32 GB, and below 16 GB one at a time and tiled as
before. A discrete card keeps
the old default, its memory still not known, and so does a machine
where no GPU adapter, or only a CPU one, answers. The bounds are 15,
23, and 30 GiB, to the nearest, since a Linux box reports a little under
what it was sold with. `GREYCARD_REMOVE_RESIDENT` (`1`, `unet`, or
`0`) and `GREYCARD_REMOVE_TILED` still win, and the log line names
what chose ("24 GiB unified: the UNet kept open, 5.1 GB held, the
decoder whole").

## Limits

- With the UNet held the whole decoder peaks near 11 GB at 1024,
  close to all three's 12.2, which is why it waits for 24 GB; under
  that the tiles keep the peak at the UNet's.
- Nothing drops the held UNet when another model loads, so the
  denoiser or SAM after a removal runs beside its 5.1 GB.
- The mode has not been run on a Mac; the numbers above are the
  overrides', and the new mode's saving is inferred from them.
- An integrated GPU on Windows sees at most half the system memory,
  which the policy does not know.
- On a machine with two GPUs "unified" follows wgpu's choice of
  adapter: `WGPU_POWER_PREF=low` picks the integrated one and reports
  unified while ONNX Runtime may run on the discrete card. It is the
  same guess the adapter's identity already makes.

## Review

A fresh review found the log calling every machine without unified
memory "a discrete card", when no adapter, or a CPU one, answers the
same way; the flag became an option with its own wording. The two-GPU
case above was its other finding. The threshold for the whole decoder
moved from 16 to 24 GB in the fix round, on the peak the held UNet
adds.
