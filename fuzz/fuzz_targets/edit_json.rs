//! One edit as JSON, migrated from whatever schema it says: whatever
//! reads writes and reads back equal.
#![no_main]

use greycard_edit::Edit;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(edit) = Edit::from_json(text) {
        greycard_fuzz::edit_round_trips(&edit, "edit");
    }
});
