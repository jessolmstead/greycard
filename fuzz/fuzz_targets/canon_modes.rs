//! The capture modes read from a Canon maker note (a CR3's CMT3 box, a
//! TIFF), which decide whether a frame takes the headroom over its
//! white. Never a panic, and the same answer twice. The note alone, not
//! a whole CR3: the box walk to it is rawler's, which the decoder has
//! already run on the same bytes, and which sizes a value by the count
//! it is given (a fuzzed count asks it for gigabytes).
#![no_main]

use greycard_core::decode::style::canon_modes_of_note;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    assert_eq!(canon_modes_of_note(data), canon_modes_of_note(data));
});
