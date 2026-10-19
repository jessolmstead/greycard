//! The edit sidecar, a `.gcd` file, as the editor loads one: whatever
//! reads writes the same JSON again after a second read.
#![no_main]

use greycard_edit::Sidecar;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(sidecar) = Sidecar::from_json(text) {
        greycard_fuzz::sidecar_round_trips(&sidecar, "sidecar");
    }
});
