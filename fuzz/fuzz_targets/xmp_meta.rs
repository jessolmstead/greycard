//! The XMP sidecar beside a frame, as the rating, flag, label,
//! keywords, title, caption, and time shift are read from it: the meta
//! read goes into an edit sidecar that reads back with it, and written
//! into a fresh packet, or back into the one it came from, it reads
//! back the same.
#![no_main]

use greycard_edit::meta::Meta;
use greycard_edit::{Sidecar, xmp};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(packet) = xmp::read(text) else {
        return;
    };
    let meta = packet.over(&Meta::default());

    let sidecar = Sidecar {
        meta: meta.clone(),
        ..Sidecar::default()
    };
    greycard_fuzz::sidecar_round_trips(&sidecar, "xmp meta in a sidecar");
    let back = Sidecar::from_json(&sidecar.to_json()).expect("read back above");
    assert_eq!(
        back.meta, meta,
        "the meta reads back from the sidecar changed"
    );

    let fresh = xmp::fresh(&meta, None);
    let again = xmp::read(&fresh)
        .unwrap_or_else(|e| panic!("a fresh packet does not read: {e}\n{fresh}"))
        .over(&Meta::default());
    assert_eq!(again, meta, "a fresh packet reads back changed\n{fresh}");

    if let Ok(written) = xmp::write(text, &meta, None) {
        let again = xmp::read(&written)
            .unwrap_or_else(|e| panic!("a rewritten packet does not read: {e}\n{written}"))
            .over(&Meta::default());
        assert_eq!(
            again, meta,
            "a rewritten packet reads back changed\n{written}"
        );
    }
});
