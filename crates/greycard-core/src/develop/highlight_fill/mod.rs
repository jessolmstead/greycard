//! The learned highlight fill, hl-1 and its successors (notes §719): a
//! CPU reference of `scripts/highlight-fill-oracle/oracle.py`, stage by
//! stage as its `SPEC.md` pins them, tested against the oracle's
//! fixtures.

pub mod analysis;
pub mod blur;
pub mod constants;
pub mod half;
pub mod harmonic;
pub mod net;
pub mod oklab;
pub mod regions;
pub mod thumbnail;

pub use constants::*;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;
