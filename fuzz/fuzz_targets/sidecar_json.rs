//! The edit sidecar, a `.gcd` file, as the editor loads one: whatever
//! reads writes the same JSON again after a second read.
#![no_main]

use greycard_edit::Sidecar;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(sidecar) = Sidecar::from_json(text, &greycard_edit::Edit::default()) {
        // A write floor past ours is a newer build's file, never saved.
        let floor = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| v["floors"]["write"].as_u64())
            .unwrap_or(0);
        if floor > u64::from(greycard_edit::VERSION) {
            assert!(
                sidecar.read_only(),
                "a write floor of {floor} reads as writable"
            );
        }
        greycard_fuzz::sidecar_round_trips(&sidecar, "sidecar");
    }
});
