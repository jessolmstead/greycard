//! Two edit sidecars of one frame, joined as the editor joins two
//! copies (§233): the input is the two files' text, a NUL byte between
//! them. Whatever reads joins both ways, with itself, and twice, as
//! `greycard_edit::sync::join_check::check` says a join must.
#![no_main]

use greycard_edit::{Edit, Sidecar};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some(at) = data.iter().position(|&b| b == 0) else {
        return;
    };
    let (Ok(a), Ok(b)) = (
        std::str::from_utf8(&data[..at]),
        std::str::from_utf8(&data[at + 1..]),
    ) else {
        return;
    };
    let read = |text: &str| Sidecar::from_json(text, &Edit::default()).ok();
    if let (Some(a), Some(b)) = (read(a), read(b)) {
        greycard_edit::sync::join_check::check(&a, &b);
    }
});
