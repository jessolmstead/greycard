"""The variance-stabilizing transform, as greycard-core's ``Vst`` has it.

With the noise model ``var = a x + b`` per channel, the generalized
Anscombe transform ``2 sqrt(x/a + 3/8 + b/a^2)`` gives every channel unit
noise. Shifted to be zero at ``x = 0`` and scaled through by ``a`` it is
``2x / (sqrt(a x + s0^2) + s0)`` with ``s0 = sqrt(3a^2/8 + b)``, which
stays finite as ``a`` goes to zero. The network sees and predicts values
in this space; ``inverse`` is the algebraic inverse, ``a d^2/4 + d s0``,
which is the right one for a value that estimates the transform of the
clean signal (the unbiased inverse of Mäkitalo and Foi is for the
transform of a noisy sample). Both must match the Rust to the bit, and
``tools/denoise/test_vst.py`` checks the closed forms round-trip.
"""

import torch


def s0(a: torch.Tensor, b: torch.Tensor) -> torch.Tensor:
    return torch.sqrt(3.0 / 8.0 * a * a + b)


def forward(x: torch.Tensor, a: torch.Tensor, b: torch.Tensor) -> torch.Tensor:
    """``a`` and ``b`` broadcast against ``x`` (per channel)."""
    s = s0(a, b)
    return 2.0 * x / (torch.sqrt(torch.clamp(a * x + s * s, min=0.0)) + s)


def inverse(d: torch.Tensor, a: torch.Tensor, b: torch.Tensor) -> torch.Tensor:
    s = s0(a, b)
    return a * d * d / 4.0 + d * s
