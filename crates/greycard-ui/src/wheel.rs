//! The hue circle the three grading wheels and the tint's ring are
//! all drawn as: where a press in it falls, and where a hue and a
//! strength put the marker.
//!
//! Pure, and here rather than in the panel's callbacks, because the
//! two directions have to be each other's inverse — a marker put
//! where a press came from, and nowhere else — and that is a thing
//! to assert rather than to look at.

/// Within this fraction of the wheel's radius the strength is
/// nothing: a dead center, so a wheel can be put back to no
/// adjustment at all without hunting for one exact pixel.
pub const DEAD: f32 = 0.04;

/// A press or a drag on a hue circle: the pointer's place in the
/// picture, 0 to 1 with y up, as a hue in degrees and a strength 0
/// to 1. The hue is `None` within the dead center, where there is no
/// angle worth reading and the control keeps the hue it had, so a
/// wheel or a ring put back to nothing does not forget it. Outside
/// the circle the strength holds at one, so a drag off the edge
/// stays at full rather than jumping.
pub fn pick(x: f32, y: f32) -> (Option<f32>, f32) {
    let (dx, dy) = (2.0 * x - 1.0, 2.0 * y - 1.0);
    let r = dx.hypot(dy);
    let hue = (r >= DEAD).then(|| dy.atan2(dx).to_degrees().rem_euclid(360.0));
    (hue, ((r - DEAD) / (1.0 - DEAD)).clamp(0.0, 1.0))
}

/// Where a hue and a strength sit in a hue circle's picture, in the
/// same coordinates: [`pick`]'s inverse for every pair it can
/// return.
pub fn place(hue: f32, strength: f32) -> (f32, f32) {
    let r = DEAD + strength.clamp(0.0, 1.0) * (1.0 - DEAD);
    let (s, c) = hue.to_radians().sin_cos();
    ((1.0 + r * c) / 2.0, (1.0 + r * s) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hue_circle_reads_an_angle_as_a_hue_and_a_radius_as_a_strength() {
        // Y is up, so the compass is the one `grading.rs` and
        // `tint.rs` use: red to the right, and round the short way.
        let deg = |x: f32, y: f32| pick(x, y).0.expect("an angle out here");
        assert!((deg(1.0, 0.5) - 0.0).abs() < 1e-3, "right is 0");
        assert!((deg(0.5, 1.0) - 90.0).abs() < 1e-3, "up is 90");
        assert!((deg(0.0, 0.5) - 180.0).abs() < 1e-3, "left is 180");
        assert!((deg(0.5, 0.0) - 270.0).abs() < 1e-3, "down is 270");

        // The center has no angle worth reading, so the hue is left
        // as it was and only the strength is answered.
        assert_eq!(pick(0.5, 0.5), (None, 0.0));
        assert_eq!(pick(0.5 + DEAD / 4.0, 0.5), (None, 0.0));

        // The strength is the radius past the dead center, and it
        // holds at one beyond the rim rather than falling off.
        let (_, half) = pick(0.5 + (DEAD + (1.0 - DEAD) / 2.0) / 2.0, 0.5);
        assert!((half - 0.5).abs() < 1e-5, "halfway out is half: {half}");
        assert_eq!(pick(1.0, 0.5).1, 1.0);
        assert_eq!(pick(1.4, 0.5).1, 1.0, "a drag off the edge stays full");
        assert_eq!(pick(0.5, -0.3).1, 1.0);

        // And the marker goes back where the press came from.
        for hue in [0.0f32, 37.5, 120.0, 215.25, 359.9] {
            for strength in [0.1f32, 0.25, 0.5, 1.0] {
                let (x, y) = place(hue, strength);
                let (back, s) = pick(x, y);
                let back = back.expect("a marker off the center has an angle");
                let apart = (back - hue).abs().min(360.0 - (back - hue).abs());
                assert!(apart < 1e-2, "{hue} came back {back}");
                assert!((s - strength).abs() < 1e-5, "{strength} came back {s}");
            }
            // At nothing the marker sits exactly on the dead edge,
            // which is the one place the inverse is not total and is
            // meant not to be: the strength comes back as nothing to
            // within rounding, and the hue either comes back as it
            // was or is not read at all — the radius lands a hair
            // either side of the edge. Both leave the control's hue
            // where it was, which is what the dead centre is for.
            let (x, y) = place(hue, 0.0);
            let (back, s) = pick(x, y);
            assert!(s < 1e-6, "{hue} at nothing came back at {s}");
            if let Some(back) = back {
                let apart = (back - hue).abs().min(360.0 - (back - hue).abs());
                assert!(apart < 1e-2, "{hue} came back {back}");
            }
        }
        // Nothing sits in the dead center, wherever its hue points,
        // and an amount out of range is brought to its end.
        assert_eq!(place(0.0, 0.0), (0.5 + DEAD / 2.0, 0.5));
        assert_eq!(place(210.0, 2.0), place(210.0, 1.0));
        assert_eq!(place(210.0, -1.0), place(210.0, 0.0));
    }
}
