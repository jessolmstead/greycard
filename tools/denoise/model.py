"""The network: a small UNet on packed Bayer, RGB out by sub-pixel shuffle.

Four channels in at half resolution (R, G1, G2, B, stabilized), three
levels down with two convolutions each, up with skips, and a head at
the mosaic's resolution. The head is where the demosaic is decided,
and it has had two forms:

- ``head="shuffle"`` (v0 to v3): the decoder's features go through one
  convolution into 4 x 8 channels, rearranged into 8 at full
  resolution, then one convolution into RGB. It leaves a faint 2 x 2
  grid on flat color, since each position has its own filters and
  the one convolution after is too little to tie them.
- ``head="mosaic"`` (v4 on): the same shuffle into 16 channels, then
  the mosaic itself, stabilized, rearranged from the packed input by
  the same shuffle, is put beside them, and two convolutions at full
  resolution make RGB from the pair. Each output pixel sees the sample
  the sensor took at its position and its neighbors, at full
  resolution, which is what a demosaic is; the decoder supplies the
  context. About the cost of one more encoder block.

About two million parameters at the default widths; every op has a
plain ONNX form (Conv, LeakyRelu, MaxPool, Resize nearest, Concat,
DepthToSpace) so ONNX Runtime's CPU and WebGPU providers both take it.
"""

import torch
import torch.nn as nn
import torch.nn.functional as F


class Block(nn.Module):
    def __init__(self, cin, cout):
        super().__init__()
        self.c1 = nn.Conv2d(cin, cout, 3, padding=1)
        self.c2 = nn.Conv2d(cout, cout, 3, padding=1)

    def forward(self, x):
        x = F.leaky_relu(self.c1(x), 0.1)
        return F.leaky_relu(self.c2(x), 0.1)


class UNet(nn.Module):
    def __init__(self, widths=(32, 64, 128, 256), cin=4, cout=3, post=8, head="shuffle"):
        super().__init__()
        self.widths = widths
        self.kind = head
        self.enc = nn.ModuleList()
        c = cin
        for w in widths:
            self.enc.append(Block(c, w))
            c = w
        self.dec = nn.ModuleList()
        for w in reversed(widths[:-1]):
            self.dec.append(Block(c + w, w))
            c = w
        self.head = nn.Conv2d(c, 4 * post, 3, padding=1)
        # ONNX Runtime's WebGPU convolution goes wrong on 33 input
        # channels (32 features and the mosaic) where 17 and 25 are
        # fine, so the mosaic is padded with zero planes to a multiple
        # of four channels; the weights on the padding are zero and
        # stay so, and the answer is the same everywhere.
        self.pad = (-(post + 1)) % 4 if head == "mosaic" else 0
        if head == "shuffle":
            self.post = nn.Conv2d(post, cout, 3, padding=1)
        elif head == "mosaic":
            self.post = nn.Sequential(
                nn.Conv2d(post + 1 + self.pad, post, 3, padding=1),
                nn.LeakyReLU(0.1),
                nn.Conv2d(post, cout, 3, padding=1),
            )
            if self.pad:
                with torch.no_grad():
                    self.post[0].weight[:, post + 1 :].zero_()
        else:
            raise ValueError(head)
        self.cout = cout

    def forward(self, x):
        mosaic = F.pixel_shuffle(x, 2)  # the four planes back to RGGB, full resolution
        skips = []
        for i, block in enumerate(self.enc):
            x = block(x)
            if i + 1 < len(self.enc):
                skips.append(x)
                x = F.max_pool2d(x, 2)
        for block in self.dec:
            x = F.interpolate(x, scale_factor=2.0, mode="nearest")
            x = block(torch.cat([x, skips.pop()], dim=1))
        x = F.leaky_relu(F.pixel_shuffle(self.head(x), 2), 0.1)
        if self.kind == "mosaic":
            parts = [x, mosaic]
            if self.pad:
                parts.append(mosaic.new_zeros(mosaic.shape[0], self.pad, *mosaic.shape[2:]))
            x = torch.cat(parts, dim=1)
        return self.post(x)

    def load_state_dict(self, state, strict=True):
        """Take a checkpoint written before the padding: its first post
        convolution has ``post + 1`` input channels, and the padded
        ones get zero weight."""
        key = "post.0.weight"
        if self.pad and key in state and state[key].shape[1] == self.post[0].in_channels - self.pad:
            state = dict(state)
            w = state[key]
            state[key] = torch.cat([w, w.new_zeros(w.shape[0], self.pad, *w.shape[2:])], dim=1)
        return super().load_state_dict(state, strict)


def parameter_count(model: nn.Module) -> int:
    return sum(p.numel() for p in model.parameters())


class Replicate(nn.Module):
    """A fixed net for the Rust tests: every output pixel of a 2x2 block
    is the block's R, its first G and its B, so the answer is known."""

    def __init__(self):
        super().__init__()
        self.head = nn.Conv2d(4, 12, 1, bias=False)
        with torch.no_grad():
            w = torch.zeros(12, 4, 1, 1)
            # Output channel order for pixel_shuffle: c * 4 + (dy * 2 + dx).
            for pos in range(4):
                w[0 * 4 + pos, 0] = 1.0  # R from R
                w[1 * 4 + pos, 1] = 1.0  # G from G1
                w[2 * 4 + pos, 3] = 1.0  # B from B
            self.head.weight.copy_(w)

    def forward(self, x):
        return F.pixel_shuffle(self.head(x), 2)


if __name__ == "__main__":
    for head, post in (("shuffle", 8), ("mosaic", 16)):
        m = UNet(post=post, head=head)
        print(head, parameter_count(m), "parameters")
        y = m(torch.zeros(1, 4, 64, 96))
        print(tuple(y.shape))
