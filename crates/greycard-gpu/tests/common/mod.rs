//! What the op tests' random sweeps share: a seeded generator, the
//! draws a slider is given, and the run that replays one case and
//! names every failing one.
//!
//! Each case is drawn from a splitmix64 generator seeded from the
//! run's seed and the case's own index, so one case is replayed from
//! those two numbers alone. `GREYCARD_OP_PARITY_SEED` and
//! `GREYCARD_OP_PARITY_CASES` change the run (decimal, or hex after
//! `0x`); `GREYCARD_OP_PARITY_ONLY` runs the one case of that index.

// Each test file is a crate of its own and takes what it needs.
#![allow(dead_code)]

use std::fmt::Debug;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

/// The run's seed when none is given: "grey", as the viewport's sweep
/// has it.
pub const SEED: u64 = 0x6772_6579;

/// splitmix64.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64, k: u64) -> Self {
        let mut r = Rng(seed ^ k.wrapping_mul(0xD1B5_4A32_D192_ED03));
        r.next();
        r
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0 to 1, 24 bits of it.
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Anywhere in `lo..hi`, uniformly.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }

    /// `0..n`.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// `lo..=hi`, uniformly.
    pub fn between(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }

    /// One of `items`.
    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }

    /// A slider over `lo..=hi`: at `lo` a fifth of the time, at `hi` a
    /// fifth, and anywhere between otherwise, since a uniform draw
    /// almost never reaches the corners and the corners are where the
    /// arithmetic breaks first.
    pub fn slider(&mut self, lo: f32, hi: f32) -> f32 {
        let u = self.unit();
        if u < 0.2 {
            lo
        } else if u >= 0.8 {
            hi
        } else {
            lo + (hi - lo) * (u - 0.2) / 0.6
        }
    }

    /// A signed slider over `lo..=hi` about zero: each end a fifth of
    /// the time, exactly zero a tenth, a small value of either sign
    /// (under a twentieth of the larger end, 0.05 on a slider of -1
    /// to 1, and mostly much less) three twentieths, where a fade near
    /// zero is decided, and anywhere between otherwise.
    pub fn signed(&mut self, lo: f32, hi: f32) -> f32 {
        let u = self.unit();
        let v = self.unit();
        if u < 0.2 {
            lo
        } else if u < 0.4 {
            hi
        } else if u < 0.5 {
            0.0
        } else if u < 0.65 {
            let small = 0.05 * lo.abs().max(hi.abs()) * v * v;
            if self.chance(0.5) { -small } else { small }
        } else {
            lo + (hi - lo) * v
        }
    }

    /// An integer slider over `lo..=hi`, its ends a fifth each.
    pub fn whole(&mut self, lo: usize, hi: usize) -> usize {
        let u = self.unit();
        if u < 0.2 {
            lo
        } else if u >= 0.8 {
            hi
        } else {
            self.between(lo, hi)
        }
    }
}

/// A run's seed and which cases it takes.
pub struct Sweep {
    pub seed: u64,
    pub cases: u64,
    pub only: Option<u64>,
}

fn env(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| {
        let v = v.trim();
        match v.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16).ok(),
            None => v.parse().ok(),
        }
    })
}

impl Sweep {
    /// The run the environment asks for, `cases` of them by default.
    pub fn from_env(cases: u64) -> Self {
        Self {
            seed: env("GREYCARD_OP_PARITY_SEED").unwrap_or(SEED),
            cases: env("GREYCARD_OP_PARITY_CASES").unwrap_or(cases),
            only: env("GREYCARD_OP_PARITY_ONLY"),
        }
    }

    /// Draw each case with `draw` and run `check` on it, which panics
    /// on a difference. Every case runs; a failing one is named with
    /// the seed, its index and what was drawn, and the run fails at
    /// the end with the list. `device` names the adapter.
    pub fn run<T: Debug>(
        &self,
        op: &str,
        device: &str,
        draw: impl Fn(&mut Rng) -> T,
        mut check: impl FnMut(&str, &T),
    ) {
        let started = Instant::now();
        let indices: Vec<u64> = match self.only {
            Some(k) => vec![k],
            None => (0..self.cases).collect(),
        };
        let mut failures = Vec::new();
        for &k in &indices {
            let drawn = draw(&mut Rng::new(self.seed, k));
            let what = format!("{op} seed {:#x} case {k}", self.seed);
            println!("{what}: {drawn:?}");
            if let Err(e) = catch_unwind(AssertUnwindSafe(|| check(&what, &drawn))) {
                let message = e
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| e.downcast_ref::<&str>().copied())
                    .unwrap_or("(no message)");
                failures.push(format!("case {k}: {drawn:?}\n    {message}"));
            }
        }
        println!(
            "{op}: {} cases from seed {:#x} on {device}, {} failed, in {:.1} s",
            indices.len(),
            self.seed,
            failures.len(),
            started.elapsed().as_secs_f64()
        );
        assert!(
            failures.is_empty(),
            "{op}: {} of {} cases from seed {:#x} failed on {device} (replay one with \
             GREYCARD_OP_PARITY_SEED={:#x} GREYCARD_OP_PARITY_ONLY=<case>):\n{}",
            failures.len(),
            indices.len(),
            self.seed,
            self.seed,
            failures.join("\n")
        );
    }
}
