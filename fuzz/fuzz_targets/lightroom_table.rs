//! A Lightroom catalog's develop settings, the Lua table text its
//! `Adobe_imageDevelopSettings.text` column keeps: the table parser,
//! the mapper with no body and with a measured one, the flat curves,
//! and the crop placed on a few frames, as the catalog import does.
//! Each preset, and each edit it makes, writes as JSON and reads back
//! equal.
#![no_main]

use greycard_core::raw::Orientation;
use greycard_edit::Edit;
use greycard_edit::lightroom::{self, crop};
use libfuzzer_sys::fuzz_target;

/// Frames the crop is placed on: as it is, turned in Lightroom and
/// here, measured with a border the catalog does not count, mirrored,
/// and a catalog that says its file has no size.
const FRAMES: [crop::Frame; 5] = [
    crop::Frame {
        size: (6000.0, 4000.0),
        catalog: Some((6000.0, 4000.0)),
        tag: Orientation::Normal,
        turn: 0,
        lightroom: None,
    },
    crop::Frame {
        size: (6000.0, 4000.0),
        catalog: Some((6048.0, 4024.0)),
        tag: Orientation::Rotate270,
        turn: 1,
        lightroom: Some(Orientation::Rotate90),
    },
    crop::Frame {
        size: (4000.0, 6000.0),
        catalog: None,
        tag: Orientation::Rotate90,
        turn: 3,
        lightroom: Some(Orientation::Normal),
    },
    crop::Frame {
        size: (6000.0, 4000.0),
        catalog: Some((6000.0, 4000.0)),
        tag: Orientation::FlipHorizontal,
        turn: 0,
        lightroom: Some(Orientation::FlipHorizontal),
    },
    crop::Frame {
        size: (6000.0, 4000.0),
        catalog: Some((0.0, 0.0)),
        tag: Orientation::Normal,
        turn: 0,
        lightroom: None,
    },
];

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = lightroom::parse_table(text);
    if let Ok(imported) = lightroom::read_table(text, "fuzz") {
        greycard_fuzz::preset_round_trips(&imported.preset, "catalog table");
        greycard_fuzz::applied_round_trips(&imported.preset, "catalog table applied");
    }
    let body = lightroom::BODY_OFFSETS[0].0;
    if let Ok(imported) = lightroom::read_table_for(text, "fuzz", Some(body)) {
        greycard_fuzz::preset_round_trips(&imported.preset, "catalog table, a body");
    }

    // The crop, as `lightroom_catalog::crop_onto` lays it.
    let words = greycard_library::lightroom_exports::words(text);
    if crop::Crop::unreadable(&words) {
        return;
    }
    let Some(c) = crop::Crop::of(&words) else {
        return;
    };
    let constrained = words
        .get("CropConstrainAspectRatio")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"));
    for frame in &FRAMES {
        if let Ok(placed) = crop::place(&c, constrained, frame) {
            let mut edit = Edit::default();
            edit.geometry = placed.applied(&edit.geometry);
            greycard_fuzz::edit_round_trips(&edit, "catalog crop");
        }
    }
});
