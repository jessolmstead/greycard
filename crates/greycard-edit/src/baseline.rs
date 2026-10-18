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
pub(crate) fn key(s: &str) -> String {
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
}
