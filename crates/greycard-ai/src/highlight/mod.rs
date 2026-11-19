//! The learned highlight fill's weights, built into the binary, one
//! constant a version (notes §719 section 5). Core's
//! `develop::highlight_fill` runs them; this module only names them.
//! It touches no ONNX Runtime, so it works where none loads.
//!
//! A version is its weights and its look constants together: a retrain
//! or a new look constant is a new name, and a name once written in an
//! edit stays in the binary, so the edit keeps its picture.
//!
//! **License.** The weights are greycard's own, GPL-3.0-or-later, as
//! the denoisers' are. They were trained only on the author's own
//! photographs, with no third-party data, and no training photograph is
//! published or recoverable from them.

use std::sync::OnceLock;

use greycard_core::develop::highlight_fill::net::{FillLook, FillWeights};

use crate::registry::License;

/// The license every version's weights are under.
pub const LICENSE: License = License {
    name: "GPL-3.0-or-later",
    url: "https://www.gnu.org/licenses/gpl-3.0.html",
};

/// Where the weights come from, for a listing to print.
pub const SOURCE: &str = "greycard's own, trained only on the author's own photographs with no third-party data; no training photograph is published or recoverable from the weights";

/// One version of the fill: its name, its weights, and its look.
#[derive(Debug)]
pub struct Version {
    /// Written in the sidecar and in the develop key's tag.
    pub name: &'static str,
    /// The look constants set at inference, which belong to the version.
    pub look: FillLook,
    /// The weights blob (`FillWeights::to_bytes`' format).
    pub blob: &'static [u8],
    /// The blob's sha256, checked in a test.
    pub sha256: &'static str,
    weights: OnceLock<FillWeights>,
}

impl Version {
    /// The weights, parsed on first use.
    ///
    /// # Panics
    /// If the built-in blob does not parse, which the tests rule out.
    pub fn weights(&self) -> &FillWeights {
        self.weights.get_or_init(|| {
            FillWeights::from_bytes(self.blob)
                .unwrap_or_else(|e| panic!("the built-in {} weights: {e}", self.name))
        })
    }
}

/// hl-1: checkpoint r6e-s1 with e_tied's look (notes §719, "What
/// ships"). Its blob is `scripts/highlight-fill-oracle/export_weights.py`
/// run on that checkpoint; the look is `oracle.py`'s constants block.
pub static HL_1: Version = Version {
    name: "hl-1",
    look: FillLook {
        core_whiten: 0.5,
        core_boost: 1.4,
        harmonic_bright: 0.25,
        depth_tie: [12.0, 48.0],
        gain_sigma: 6.0,
    },
    blob: include_bytes!("hl-1.bin"),
    sha256: "4ed0d07ae594d0dd88f82cef5f87ca1a597303e86b0e8e1ac6abeca07f105091",
    weights: OnceLock::new(),
};

/// Every version this build carries, oldest first.
pub static VERSIONS: [&Version; 1] = [&HL_1];

/// The version an edit names, if this build has it.
pub fn version(name: &str) -> Option<&'static Version> {
    VERSIONS.iter().copied().find(|v| v.name == name)
}

#[cfg(test)]
mod tests;
