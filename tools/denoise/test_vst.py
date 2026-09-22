import torch

from vst import forward, inverse


def test_round_trip():
    a = torch.tensor([3e-4, 1e-2, 0.0])
    b = torch.tensor([5e-8, 1e-4, 1e-4])
    x = torch.linspace(0, 1.2, 1000)[:, None]
    d = forward(x, a, b)
    assert torch.allclose(inverse(d, a, b), x.expand_as(d), atol=1e-6)


def test_unit_noise():
    torch.manual_seed(0)
    a, b = torch.tensor(1e-3), torch.tensor(1e-5)
    for level in (0.01, 0.1, 0.5):
        x = torch.full((1_000_000,), level)
        noisy = a * torch.poisson(x / a) + torch.sqrt(b) * torch.randn_like(x)
        assert abs(forward(noisy, a, b).std().item() - 1.0) < 0.02


if __name__ == "__main__":
    test_round_trip()
    test_unit_noise()
    print("ok")
