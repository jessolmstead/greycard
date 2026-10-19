//! A RawTherapee profile, a `.pp3`: read for a frame of no known size
//! and for one with a size, carrying what it sets and carrying every
//! section. The preset it makes and the edit laid onto a fresh one
//! each write as JSON and read back equal.
#![no_main]

use greycard_edit::Edit;
use greycard_edit::pp3::{self, Carry, Frame};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let frames = [
        Frame::default(),
        Frame {
            size: Some((6000, 4000)),
            turned: false,
        },
    ];
    for frame in frames {
        for carry in [Carry::Set, Carry::Every] {
            let Ok(imported) = pp3::read_carrying(text, frame, carry) else {
                return;
            };
            greycard_fuzz::preset_round_trips(&imported.preset, "pp3");
            let edit = imported.applied(&Edit::default());
            greycard_fuzz::edit_round_trips(&edit, "pp3 applied");
            let _ = imported.report();
        }
    }
});
