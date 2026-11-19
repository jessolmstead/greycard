//! The baseline: the stops a raw is brightened by before its own
//! exposure, per body.
//!
//! The Exposure slider's zero is the default develop, and the default
//! develop of a raw is the scene brightened by its body's baseline
//! (the finish adds it where the exposure turns into a gain). One
//! number served every body until the default develop was measured
//! against each camera's own JPEG over a library (`greycard-ui
//! --measure-baseline`): the bodies differ, and a body whose median
//! sits apart from the bodies the Lightroom comparisons found right at
//! [`DEFAULT`] by more than a body's own spread gets its own number
//! here. This is the structure of Adobe's `BaselineExposure`, one
//! number a body.
//!
//! An edit written before the bodies had numbers of their own was made
//! against [`BEFORE_BODIES`] whatever its body, and keeps it
//! ([`crate::Light::baseline`], set by [`crate::migrate`]), so a
//! picture someone finished does not move under them.
//!
//! A DNG's own `BaselineExposure` is carried on the frame and not yet
//! applied: [`for_frame`] gives every frame its body's number. The tag
//! is additive on Adobe's scale, and the numbers here were fitted
//! against Lightroom renders that already carried Adobe's own baseline
//! for the reference bodies. So the rule to come is a neutral (these
//! numbers less Adobe's baseline for those bodies) plus the file's
//! tag; putting the tag in place of [`DEFAULT`] would open a DNG that
//! Adobe's converter made from a Canon raw about half a stop darker
//! than the raw itself. What has to be measured first: one
//! Adobe-converted DNG of a reference body (an R5 Mark II or an R6
//! Mark II), its `BaselineExposure` read, which is the neutral's
//! offset. Until then a phone's DNG, whose tag says its burst is
//! stored dark, opens darker than the phone's picture; and once the
//! rule is in, the Lightroom import's exposure conversion, which goes
//! by [`for_body`], is off for a DNG by the same difference. A DCP's
//! `BaselineExposureOffset`, which the specification adds on top of
//! the file's tag, is read and not applied either; it comes after.

/// The baseline of a body not in [`BODIES`], in stops: where the
/// first reference frames put a Canon's default against Lightroom's
/// and the camera's JPEG.
pub const DEFAULT: f32 = 0.8;

/// The baseline every edit was made against before [`BODIES`]: what a
/// sidecar written then keeps.
pub const BEFORE_BODIES: f32 = 0.8;

/// The bodies whose default develop sits apart from the rest against
/// their camera's own JPEG, and their baseline in stops: [`DEFAULT`]
/// plus the median over the body's sessions (a folder each, the median
/// of its frames without an adaptive setting) of the stops the
/// Exposure slider moves to land the default develop on the camera's
/// JPEG, less the same median over the R5 Mark II's and R6 Mark II's
/// sessions, the bodies Lightroom's own exports put at [`DEFAULT`].
/// A body is here only when that difference, and the same over frames
/// rather than sessions, are both past 0.15 stops on one side, from
/// five sessions or more. The GFX 100S II: 32 frames in 6 sessions,
/// -0.28 by session and -0.18 by frame. The names are as the library
/// names a camera (`camera_name`), compared with case, spaces and
/// punctuation dropped.
pub const BODIES: &[(&str, f32)] = &[("Fujifilm GFX 100S II", 0.52)];

/// A body's name as the tables compare it: case, spaces and
/// punctuation dropped.
pub fn key(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The baseline for a raw of `camera` (as the library names a body);
/// [`DEFAULT`] for none or one not in [`BODIES`].
pub fn for_body(camera: Option<&str>) -> f32 {
    let Some(camera) = camera else {
        return DEFAULT;
    };
    let want = key(camera);
    BODIES
        .iter()
        .find(|(name, _)| key(name) == want)
        .map_or(DEFAULT, |(_, b)| *b)
}

/// [`for_body`] from a raw's make and model as the decoder gives them.
pub fn for_make_model(make: &str, model: &str) -> f32 {
    for_body(Some(&greycard_core::raw::camera_name(make, model)))
}

/// The baseline of a decoded raw: its body's ([`for_make_model`]) for
/// now, whatever its DNG carries. The seam where the file's tag will
/// join it, by the rule in the module's doc, once its neutral is
/// measured. When the file's BaselineExposure is honored here, a
/// merge's is the reference frame's carried over, and its
/// [`headroom`] must not be added on top of it a second time.
pub fn for_frame(frame: &greycard_core::RawFrame) -> f32 {
    for_make_model(&frame.make, &frame.model)
}

/// The stops below our baseline that a raw carrying no
/// `BaselineExposure` of its maker's (every format but DNG) is read at
/// under a camera profile's own tone curve, since Adobe's own numbers
/// for those bodies are inside its converter. Measured, not derived:
/// matching mid grey through Adobe Standard's curve gives 0.53, but
/// against the cameras' own JPEGs that left three Olympus frames 1.9
/// to 3.0 L* dark, and 0.37 centers them. The one number: the curve
/// and the profile's look table both read it ([`adobe_lift`]).
pub const ADOBE_GREY_LIFT: f32 = 0.37;

/// A DNG's own `BaselineExposure`, the maker's word for the camera, in
/// stops: none for a file without the tag, and none for one of ours (a
/// merge, a linear DNG of a develop), whose tag is our headroom.
/// Held to ten stops either way.
pub fn makers(frame: &greycard_core::RawFrame) -> Option<f32> {
    frame
        .dng
        .baseline_exposure
        .filter(|s| s.is_finite() && frame.dng.merged.is_none() && !frame.dng.ours)
        .map(|s| s.clamp(-10.0, 10.0))
}

/// The stops a camera profile's own tone curve, and the profile's look
/// table before it, read the picture at over our baseline `ours`:
/// Adobe's `BaselineExposure` less ours. A DNG's own tag ([`makers`])
/// is Adobe's; a raw without one stands in with ours less
/// [`ADOBE_GREY_LIFT`].
pub fn adobe_lift(ours: f32, makers: Option<f32>) -> f32 {
    makers.map_or(-ADOBE_GREY_LIFT, |adobe| adobe - ours)
}

/// The stops a merge of ours stored its samples under the reference
/// frame's scale, as its own tag says; nothing for any other file. An
/// exposure on top of the baseline, so the merge opens at the
/// reference's brightness, and not part of the baseline: the display
/// white follows the body's alone, and the merge renders as its
/// reference did with the range over it left for the Highlights.
pub fn headroom(frame: &greycard_core::RawFrame) -> f32 {
    frame.dng.merged.as_ref().map_or(0.0, |m| m.headroom_stops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_in_the_table_takes_its_own() {
        let gfx = BODIES[0].1;
        assert_eq!(for_body(Some("Fujifilm GFX 100S II")), gfx);
        assert_eq!(for_body(Some("fujifilm gfx100s ii")), gfx);
        assert_eq!(for_make_model("FUJIFILM", "GFX100S II"), gfx);
    }

    #[test]
    fn a_body_not_in_the_table_keeps_the_default() {
        assert_eq!(for_body(Some("Canon EOS R5 Mark II")), DEFAULT);
        assert_eq!(for_body(Some("Canon EOS R6 Mark II")), DEFAULT);
        assert_eq!(for_body(Some("Some Other Body")), DEFAULT);
        assert_eq!(for_body(None), DEFAULT);
        assert_eq!(DEFAULT, BEFORE_BODIES);
    }

    /// Until its neutral is measured, a DNG's own baseline is carried
    /// and the frame takes its body's all the same.
    #[test]
    fn a_frame_takes_its_bodys_baseline_whatever_its_dng_carries() {
        use greycard_core::raw::{LevelPattern, Levels, Orientation, Samples, SensorLayout};
        let mut frame = greycard_core::RawFrame {
            make: "FUJIFILM".into(),
            model: "GFX100S II".into(),
            other_names: Vec::new(),
            width: 1,
            height: 1,
            channels: 1,
            layout: SensorLayout::Monochrome,
            samples: Samples::U16(vec![0]),
            levels: Levels {
                black: LevelPattern::uniform(0.0, 1),
                white: LevelPattern::uniform(1.0, 1),
                black_deltas: None,
                saturation: None,
            },
            as_shot_coefficients: None,
            calibrations: Vec::new(),
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
            dng: Default::default(),
        };
        assert_eq!(for_frame(&frame), BODIES[0].1);
        frame.dng.baseline_exposure = Some(0.9);
        assert_eq!(for_frame(&frame), BODIES[0].1);
        // A merge of ours says the stops it stored its samples under,
        // apart from the baseline.
        assert_eq!(headroom(&frame), 0.0);
        frame.dng.merged = Some(greycard_core::merge::Tag {
            kind: "hdr".into(),
            headroom_stops: 2.5,
            ..Default::default()
        });
        assert_eq!(for_frame(&frame), BODIES[0].1);
        assert_eq!(headroom(&frame), 2.5);
    }
}
