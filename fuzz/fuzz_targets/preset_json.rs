//! A preset file, a `.gcp`, which people pass around: whatever reads
//! writes and reads back equal, and lays onto an edit that does too.
#![no_main]

use greycard_edit::Preset;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(preset) = Preset::from_json(text) {
        greycard_fuzz::preset_round_trips(&preset, "preset");
        greycard_fuzz::applied_round_trips(&preset, "preset applied");
    }
});
