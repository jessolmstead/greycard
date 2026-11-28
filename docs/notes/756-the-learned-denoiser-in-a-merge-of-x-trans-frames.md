# §756. The learned denoiser in a merge of X-Trans frames (2026-11-27)

§731 gave a single X-Trans frame its tier's X-Trans network and left
a merge on Bayer alone: a merge of X-Trans frames demosaiced them
plain and said so. Only two places held that. The merge's frame list
named the tier's Bayer network whatever the mosaic, and the check
before a merge is made again refused any frame that was not Bayer.

**The frame's list picks the network.** A merge names a tier, and
each frame's list runs the network its mosaic has, as a develop does
(`greycard_ai::for_mosaic`). A merge of both kinds of frame runs both
networks. The skip for X-Trans and its words are gone; the skip for a
card the denoiser lost stays, on either mosaic.

**Merging again.** The check reads each frame's mosaic and asks the
store for every network the frames run, so a tier with only its Bayer
network fetched refuses before anything is written. A frame that is
neither Bayer nor X-Trans is refused as before.

**Brightness.** A denoised X-Trans merge can record a
BaselineExposure about 0.1 stop under the plain one's. The plain
merge's brightest sample is a noise spike, which the denoiser
removes; both files render alike, and only Auto may differ.

## Measured

A bracket re-mosaiced to X-Trans from Bayer raws, since we have no
X-Trans bracket: each tier merged on the RTX 5070 Ti in 7 to 10 s. On lavapipe Dawn fails inside the X-Trans
network, in a develop as in a merge, so the tests there use the plain
denoiser.

## Left open

- A real X-Trans bracket, and a panorama.
- A focus stack's memory under the learned denoiser is charged at
  Bayer's 190 bytes a pixel; X-Trans is unmeasured.
