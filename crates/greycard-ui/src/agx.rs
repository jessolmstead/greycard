//! AgX as a display transform, Blender's wide-gamut formation with its
//! "Punchy" look as the default: the working-space color's negative
//! channels offset away at held luminance, its primaries inset toward
//! white and rotated by a matrix, a log encoding over sixteen and a
//! half stops, one sigmoid per channel to a 2.4-encoded value, the
//! decode, the hue partly put back to what it was before the curve,
//! and an outset matrix; then display-linear Rec.2020, where the point
//! curves, the look table and the output transform take it as they
//! take the curve per channel's output. For an output narrower than
//! Rec.2020 the finish's output stage applies the formation's guard
//! rail in that space in place of a plain clip (`output_rail`).
//!
//! Ported from darktable's AgX module, `src/iop/agx.c`,
//! GPL-3.0-or-later, written by Kofa in 2025; copyright (C) 2025-2026
//! darktable developers. Taken from it: the compression of a color
//! outside the base gamut (`_compress_into_gamut`); the log encoding
//! relative to mid grey over a black and a white relative exposure
//! (`_apply_log_encoding`); the sigmoid with a toe and a shoulder of
//! their own powers about a pivot, each scaled so the curve passes
//! through the range's ends (`_sigmoid`, `_scale`, `_scaled_sigmoid`,
//! `_calculate_tone_mapping_params`); the decode by the curve's gamma;
//! and the mix of the input's HSV hue back into the output's
//! (`_lerp_hue`, its "preserve hue"). The module's defaults are
//! Blender's, which is what is used here: the inset, rotation and
//! outset of Eary Chow's formation (github.com/EaryChow/AgX, the
//! generator AgX_LUT_Gen, on Troy Sobotka's AgX), a slope of 2.4 with
//! toe and shoulder powers of 1.5 about mid grey at an encoded
//! 0.18^(1/2.4), a gamma of 2.4, and 40 percent of the per-channel
//! hue shift kept. The sigmoid is Jed Smith's, as all three credit it.
//! The matrices are taken as numbers: the inset as Blender's config
//! writes it, the outset as the generator's construction gives it
//! (tools/agx-oracle.py's companion model reproduces the inset from
//! its parameters to 1e-8 and the outset by the same construction).
//!
//! The look is Blender's "AgX - Punchy", not darktable's CDL-style
//! sliders: in the config it is a shadows tone grade and a power of
//! 1.0912 in the 25-stop "AgX Log" encoding, the inset space's log.
//! The shadows grade is OpenColorIO's `GradingToneTransform`, whose
//! per-channel shadows curve, `FauxCubicFwdEval` and `ComputeHSFwd` in
//! `src/OpenColorIO/ops/gradingtone`, is ported as [`Shadows`], for the
//! forward case only (a value under one), which is the look's. That
//! code is under the BSD-3-Clause license, whose notice follows and
//! covers [`Shadows`] here and `agx_shadows` in `viewport.wgsl`:
//!
//! > Copyright Contributors to the OpenColorIO Project.
//! >
//! > Redistribution and use in source and binary forms, with or
//! > without modification, are permitted provided that the following
//! > conditions are met:
//! >
//! > 1. Redistributions of source code must retain the above copyright
//! >    notice, this list of conditions and the following disclaimer.
//! > 2. Redistributions in binary form must reproduce the above
//! >    copyright notice, this list of conditions and the following
//! >    disclaimer in the documentation and/or other materials provided
//! >    with the distribution.
//! > 3. Neither the name of the copyright holder nor the names of its
//! >    contributors may be used to endorse or promote products derived
//! >    from this software without specific prior written permission.
//! >
//! > THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
//! > "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
//! > LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS
//! > FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
//! > COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT,
//! > INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
//! > (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
//! > SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
//! > HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
//! > CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR
//! > OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE,
//! > EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//!
//! The guard rails descend from one function, darktable's
//! `_compress_into_gamut` (GPL), which is Eary Chow's luminance
//! compensation for Rec.2020 as darktable ported it; Eary Chow is
//! named for the method, and his repository, which carries no
//! license, is not a source of any code here. The lower rail
//! ([`compensate_low_side`]) is that function with its luminance
//! weights. The output rail for a narrower display
//! ([`output_rail`]) is the same function applied in the output
//! space, its luminance weights those of Rec.2020 folded through the
//! output's matrix. Blender's sRGB generator has a further step in
//! its rail, a lerp of the luminance toward the opponent-compensated
//! one by its 0.08 power, which is not in darktable and is not here;
//! what its absence costs against Blender's shipped sRGB LUT at the
//! full node grid is measured in the notes and pinned at the sampled
//! nodes by `the_srgb_formation_at_the_luts_nodes`.
//!
//! Departures from darktable. The transform runs in the working space
//! and the base space both Rec.2020, which is where Blender's matrices
//! are defined, so the module's base-profile matrices are identities
//! here. The white relative exposure is not Blender's 6.5 stops over
//! mid grey but what darktable's auto-white picker would set for this
//! pipeline: the sensor's clip plus the baseline lands at display
//! white (`blender_punchy` says how it is found). darktable's linear
//! section between toe and shoulder is of zero length, as in Blender;
//! its fallback toe and shoulder for a slope too shallow to reach the
//! ends are not ported (a debug assertion says when they would be
//! wanted). The output guard rail, which darktable does not have, is
//! here since the oracle shows the plain clip off by up to a hundred
//! levels on a color outside sRGB. The formation is clipped to the
//! Rec.2020 cube before the output matrix and that rail, which
//! Blender's sRGB and P3 generators do not do; the point curves and
//! the look table between the curve and the output expect the cube,
//! and the departure is measured and pinned in
//! `the_srgb_formation_at_the_luts_nodes`.

use std::sync::LazyLock;

use greycard_core::color::apply3;

type Matrix3 = [[f32; 3]; 3];

/// Mid grey in the scene, where the log encodings are anchored.
const MID_GREY: f64 = 0.18;
/// darktable's `_epsilon`: the guard on the scale's divisions.
const EPSILON: f64 = 1e-6;

/// Jed Smith's sigmoid about a pivot: a toe and a shoulder, each
/// `scale * t / (1 + t^power)^(1/power)` of the slope's run from the
/// pivot, scaled so the curve reaches (0, 0) and (1, 1) exactly, with
/// no linear section between them. `x` is the log-encoded scene in the
/// unit range; the value is the encoded display, one at `x` of one and
/// rising past it, as the generator's curve does, so a channel over
/// the range's top keeps pulling on the others through the outset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sigmoid {
    pub pivot_x: f32,
    pub pivot_y: f32,
    pub slope: f32,
    pub toe_power: f32,
    pub shoulder_power: f32,
    pub toe_scale: f32,
    pub shoulder_scale: f32,
}

impl Sigmoid {
    /// A sigmoid through `(pivot_x, pivot_y)` with `slope` there, its
    /// toe and shoulder of the given powers. darktable's
    /// `_calculate_tone_mapping_params` with the linear ratios at zero,
    /// the target black at zero and the target white at one.
    pub fn new(
        pivot_x: f32,
        pivot_y: f32,
        slope: f32,
        toe_power: f32,
        shoulder_power: f32,
    ) -> Self {
        let (px, py, m) = (f64::from(pivot_x), f64::from(pivot_y), f64::from(slope));
        // The toe is the shoulder's calculation flipped left for right
        // and up for down, as the module does it.
        let toe_scale = -scale(1.0, 1.0, 1.0 - px, 1.0 - py, m, f64::from(toe_power));
        let shoulder_scale = scale(1.0, 1.0, px, py, m, f64::from(shoulder_power));
        let sigmoid = Self {
            pivot_x,
            pivot_y,
            slope,
            toe_power,
            shoulder_power,
            toe_scale: toe_scale as f32,
            shoulder_scale: shoulder_scale as f32,
        };
        debug_assert!(
            sigmoid.reaches_both_ends(),
            "a sigmoid too shallow for its chords wants darktable's fallback, not ported"
        );
        sigmoid
    }

    /// Whether the slope is steep enough on both sides for the sigmoid
    /// to reach the range's ends; where it is not, darktable falls
    /// back to a power curve, which is not ported.
    pub fn reaches_both_ends(&self) -> bool {
        let toe_chord = self.pivot_y / self.pivot_x;
        let shoulder_chord = (1.0 - self.pivot_y) / (1.0 - self.pivot_x);
        toe_chord <= self.slope && shoulder_chord <= self.slope
    }

    /// The curve at `x` at or over zero: zero at zero, one at one and
    /// rising past it.
    #[inline]
    pub fn at(&self, x: f32) -> f32 {
        let (scale, power) = if x < self.pivot_x {
            (self.toe_scale, self.toe_power)
        } else {
            (self.shoulder_scale, self.shoulder_power)
        };
        // The run from the pivot in the scale's units: never negative,
        // since the toe's scale is negative and its x under the pivot.
        let t = (self.slope * (x - self.pivot_x) / scale).max(0.0);
        // `t^1.5` as `t * sqrt(t)`, which the shader does too.
        let tp = if power == 1.5 {
            t * t.sqrt()
        } else {
            t.powf(power)
        };
        let s = t / (1.0 + tp).powf(1.0 / power);
        (scale * s + self.pivot_y).max(0.0)
    }
}

/// darktable's `_scale`: the scale that takes the sigmoid from the
/// transition at `(tx, ty)` with `slope` to exactly `(limit_x,
/// limit_y)`; `(actual rise)^-p - (projected rise)^-p` to the `-1/p`,
/// in its own words, the projected rise being the linear section's if
/// it ran to the limit.
fn scale(limit_x: f64, limit_y: f64, tx: f64, ty: f64, slope: f64, power: f64) -> f64 {
    let projected = slope * (limit_x - tx).max(EPSILON);
    let actual = (limit_y - ty).max(EPSILON);
    let base = (actual.powf(-power) - projected.powf(-power)).max(EPSILON);
    base.powf(-1.0 / power).min(1e9)
}

/// OpenColorIO's shadows tone grade on one channel, forward: a faux
/// cubic from `(x0, x0)` with slope `m0` to `(x2, x2)` with slope one,
/// the line of slope `m0` under `x0` and the identity over `x2`. The
/// look's shadows are 0.2 on each channel and 0.35 on the master, with
/// start 0.4 and pivot 0.1, which OCIO's precompute makes `x0` 0.1 and
/// `x2` 0.4.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadows {
    x0: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    m0: f32,
}

impl Shadows {
    pub fn new(pivot: f32, start: f32, value: f32) -> Self {
        let (x0, x2, m0, m2) = (pivot, start, value.max(0.01), 1.0f32);
        let x1 = x0 + (x2 - x0) * 0.5;
        let (y0, y2) = (x0, x2);
        let y1 = (0.5 / (x2 - x0))
            * ((2.0 * y0 + m0 * (x1 - x0)) * (x2 - x1) + (2.0 * y2 - m2 * (x2 - x1)) * (x1 - x0));
        Self { x0, x1, x2, y1, m0 }
    }

    #[inline]
    pub fn at(&self, t: f32) -> f32 {
        let (x0, x1, x2, y1, m0) = (self.x0, self.x1, self.x2, self.y1, self.m0);
        let (y0, y2, m2) = (x0, x2, 1.0);
        if t < x0 {
            return y0 + (t - x0) * m0;
        }
        if t > x2 {
            return y2 + (t - x2) * m2;
        }
        if t < x1 {
            let tl = (t - x0) / (x1 - x0);
            y0 * (1.0 - tl * tl) + y1 * tl * tl + m0 * (1.0 - tl) * tl * (x1 - x0)
        } else {
            let tr = (t - x1) / (x2 - x1);
            y1 * (1.0 - tr) * (1.0 - tr) + y2 * (2.0 - tr) * tr + m2 * (tr - 1.0) * tr * (x2 - x1)
        }
    }
}

/// Rec.2020's luminance weights the rails read: darktable's
/// `_compress_into_gamut` constants, which are the set Blender's
/// shipped formation LUTs were made with (the generator's later set,
/// 0.2589, 0.6105, 0.1306, matches neither the LUTs nor darktable; at
/// its nodes the Rec.2020 LUT is off by 7.8e-3 with it and 7e-8 with
/// these).
pub const LUMA_2020: [f32; 3] = [0.265_818_04, 0.598_469_86, 0.135_712_1];

/// The lower guard rail, darktable's `_compress_into_gamut` (Eary
/// Chow's method, Blender's lower rail): a color with a channel under
/// zero is offset until its lowest channel is zero, then scaled so its
/// luminance, measured with a correction for the negative part by the
/// opponent color, is what it was. A color with no channel under zero
/// comes back exactly. `weights` are the luminance weights of the
/// space the color is in.
#[inline]
pub fn compensate_low_side(c: [f32; 3], weights: [f32; 3]) -> [f32; 3] {
    let lo = c[0].min(c[1]).min(c[2]);
    if lo >= 0.0 {
        return c;
    }
    let dot = |v: [f32; 3]| weights[0] * v[0] + weights[1] * v[1] + weights[2] * v[2];
    let hi = c[0].max(c[1]).max(c[2]);
    let opponent = c.map(|v| hi - v);
    let y_compensated = opponent.iter().fold(0.0f32, |a, &v| a.max(v)) - dot(opponent) + dot(c);
    let offset = c.map(|v| v - lo);
    let hi2 = offset[0].max(offset[1]).max(offset[2]);
    let opponent2 = offset.map(|v| hi2 - v);
    let y_new = opponent2.iter().fold(0.0f32, |a, &v| a.max(v)) - dot(opponent2) + dot(offset);
    let ratio = if y_new > y_compensated && y_new > EPSILON as f32 {
        y_compensated / y_new
    } else {
        1.0
    };
    offset.map(|v| v * ratio)
}

/// The output guard rail for a display narrower than Rec.2020: the
/// lower rail, darktable's function, applied in the output space with
/// `weights` the luminance weights of Rec.2020 folded through the
/// output's matrix ([`rail_weights_for`]). `c` is the color in the
/// output space; a color with no channel under zero comes back
/// exactly. Blender's sRGB generator lerps the luminance toward the
/// compensated one by its 0.08 power on both sides; that step is not
/// darktable's and is not taken, and the notes say what it costs.
#[inline]
pub fn output_rail(c: [f32; 3], weights: [f32; 3]) -> [f32; 3] {
    compensate_low_side(c, weights)
}

/// The luminance weights [`output_rail`] reads for an output space:
/// Rec.2020's weights through the matrix from that space back to
/// Rec.2020, `from_out` in rows. For Rec.2020 itself they are
/// [`LUMA_2020`]; the formation's output is clipped to the Rec.2020
/// cube before the output matrix (`form`), so for that output nothing
/// reaches the rail under zero and it never acts.
pub fn rail_weights(from_out: &Matrix3) -> [f32; 3] {
    [0, 1, 2].map(|k| {
        LUMA_2020[0] * from_out[0][k]
            + LUMA_2020[1] * from_out[1][k]
            + LUMA_2020[2] * from_out[2][k]
    })
}

/// HSV's hue in turns (0 to 1), value and saturation, as `colour` and
/// darktable compute them.
#[inline]
fn hsv(c: [f32; 3]) -> (f32, f32, f32) {
    let hi = c[0].max(c[1]).max(c[2]);
    let lo = c[0].min(c[1]).min(c[2]);
    let d = hi - lo;
    if d <= 0.0 {
        return (0.0, 0.0, hi);
    }
    let h = if hi == c[0] {
        (c[1] - c[2]) / d
    } else if hi == c[1] {
        2.0 + (c[2] - c[0]) / d
    } else {
        4.0 + (c[0] - c[1]) / d
    } / 6.0;
    let s = if hi == 0.0 { 0.0 } else { d / hi };
    (h.rem_euclid(1.0), s, hi)
}

#[inline]
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h6 = h * 6.0;
    let i = (h6.floor() as i32).rem_euclid(6);
    let f = h6 - h6.floor();
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// `h1` moved toward `h2` by `t`, the short way round the circle.
#[inline]
fn lerp_hue(h1: f32, h2: f32, t: f32) -> f32 {
    let mut d = h2 - h1;
    if d > 0.5 {
        d -= 1.0;
    } else if d < -0.5 {
        d += 1.0;
    }
    (h1 + t * d).rem_euclid(1.0)
}

/// The transform: its matrices, its log ranges, its curve and its look.
#[derive(Debug, Clone, PartialEq)]
pub struct Agx {
    /// Blender's inset of the Rec.2020 primaries toward white, rotated:
    /// the generator's rotation [2.14, -1.23, -3.05] degrees and inset
    /// [0.3297, 0.2805, 0.1248], as the config's "AgX Log" writes it.
    pub inset: Matrix3,
    inset_inverse: Matrix3,
    /// Blender's outset after the curve: the generator's [0.3232,
    /// 0.2833, 0.0374] with no rotation, inverted.
    pub outset: Matrix3,
    /// `log2` of the scene value at the bottom of the curve's range,
    /// and the range's width in stops: ten under mid grey to the white
    /// relative exposure over it, found at construction so the
    /// sensor's clip lands at display white (Blender's is 6.5).
    log_min: f32,
    pub log_range: f32,
    /// The look's log: Blender's "AgX Log", 25 stops from ten under mid
    /// grey.
    look_log_min: f32,
    look_log_range: f32,
    pub curve: Sigmoid,
    /// The encoding the curve's output is in: decoded by this power.
    pub gamma: f32,
    /// How much of the per-channel hue shift is kept, 0.4: the input's
    /// HSV hue is put back by the rest.
    pub hue_kept: f32,
    /// The look: the shadows grade on each channel, then on all three
    /// as the master, then the power, all in the look's log.
    pub look_shadows: Shadows,
    pub look_master: Shadows,
    pub look_power: f32,
    /// A gain on the scene before the chain, inside this mode only,
    /// that puts mid grey where the curve per channel puts it: the
    /// switch compares a rendering, not a brightness. Found at
    /// construction against `finish::tone`; the doc on
    /// `blender_punchy` has the figure.
    pub gain: f32,
}

/// Blender's inset matrix, rows, from its config.
const BLENDER_INSET: Matrix3 = [
    [0.856_627_15, 0.095_121_24, 0.048_251_606],
    [0.137_319, 0.761_242, 0.101_439_04],
    [0.111_898_21, 0.076_799_42, 0.811_302_37],
];
/// Blender's outset matrix, rows, from the generator's construction
/// (tools/agx-oracle.py's companion model).
const BLENDER_OUTSET: Matrix3 = [
    [1.127_100_6, -0.110_606_64, -0.016_493_931],
    [-0.141_329_75, 1.157_823_7, -0.016_493_931],
    [-0.141_329_75, -0.110_606_64, 1.251_936_4],
];

impl Agx {
    /// Blender's AgX with its Punchy look, set to this pipeline at two
    /// points, both found at construction and held by tests. Mid grey:
    /// a gain before the chain, inside this mode only, puts scene mid
    /// grey at the display-linear value the curve per channel gives
    /// it, 0.267 (as the config has it the base puts 0.18 at 0.18 and
    /// Punchy at 0.097, 1.46 display stops under), since a tester
    /// throwing the switch should see a rendering change and not an
    /// exposure change. White: the white relative exposure, darktable's
    /// `range_white_relative_ev` and what its auto-white picker sets,
    /// is where a neutral at the sensor's clip plus the baseline
    /// (`finish::DISPLAY_WHITE`, 3.27 stops over mid grey) just reaches
    /// display white; with Blender's 6.5 it rendered at 0.759, 226 of
    /// 255, a flat grey clip on every blown highlight, which per
    /// channel puts at 255 (§145 fixed exactly that). The two depend on
    /// each other (the pivot moves with the range and the gain with the
    /// pivot), so the white is bisected with the gain found inside each
    /// step. The shader's literals for both and for what follows from
    /// them are held to these by `mid_grey_through_agx_is_per_channels`,
    /// which prints them.
    pub fn blender_punchy() -> Self {
        Self::fitted(false)
    }

    /// The second construction, a look call left open for the user:
    /// the shorter range at Blender's slope makes every stop about 19
    /// percent steeper, so here the slope is scaled by the range over
    /// Blender's 16.5 stops, about 2.02, keeping Blender's contrast per
    /// stop with white at the clip. Not a user control: chosen by the
    /// `GREYCARD_AGX_SOFT` environment variable for the comparison
    /// renders, and the shader's literals are the first construction's.
    pub fn blender_punchy_soft() -> Self {
        Self::fitted(true)
    }

    /// The white relative exposure found: the largest at which the
    /// sensor's clip still renders white (under it the clip is past
    /// the curve's top and clipped to one, over it the curve has not
    /// got there), with the gain found inside each step; the slope
    /// Blender's, or scaled by the range if `soft`.
    fn fitted(soft: bool) -> Self {
        let white = [crate::finish::DISPLAY_WHITE; 3];
        // The bracket's bottom is where the sigmoid still reaches its
        // ends: the shoulder's chord, `(1 - pivot_y) / (1 - pivot_x)`,
        // is `(1 - 0.489) (w + 10) / w`, under Blender's slope of 2.4
        // from 2.7 stops up and under the scaled slope from 3.5 up.
        let (mut lo, mut hi) = (if soft { 3.6f64 } else { 2.8f64 }, 6.5f64);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if Self::with_white(mid, true, soft).apply(white)[1] >= 1.0 - 1e-6 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Self::with_white(0.5 * (lo + hi), true, soft)
    }

    /// Blender's AgX as Blender ships it: the white relative exposure
    /// 6.5 and no gain, which is what the oracle fixture holds the
    /// formation to; the pipeline's is [`Agx::blender_punchy`].
    #[cfg(test)]
    pub fn blender() -> Self {
        Self::with_white(6.5, false, false)
    }

    /// The transform with the curve's range topping out `white_ev`
    /// stops over mid grey, with the gain that holds mid grey if
    /// `gained`, found by bisection on `tone(0.18)`, and the slope
    /// scaled by the range over 16.5 if `soft`.
    fn with_white(white_ev: f64, gained: bool, soft: bool) -> Self {
        {
            let black_ev = -10.0f64;
            let slope = if soft {
                2.4 * (white_ev - black_ev) / 16.5
            } else {
                2.4
            };
            let mut agx = Self {
                inset: BLENDER_INSET,
                inset_inverse: greycard_core::color::invert3(BLENDER_INSET)
                    .expect("the inset inverts"),
                outset: BLENDER_OUTSET,
                log_min: (MID_GREY.log2() + black_ev) as f32,
                log_range: (white_ev - black_ev) as f32,
                look_log_min: -12.473_93,
                look_log_range: 12.526_069 + 12.473_93,
                curve: Sigmoid::new(
                    (-black_ev / (white_ev - black_ev)) as f32,
                    MID_GREY.powf(1.0 / 2.4) as f32,
                    slope as f32,
                    1.5,
                    1.5,
                ),
                gamma: 2.4,
                hue_kept: 0.4,
                look_shadows: Shadows::new(0.1, 0.4, 0.2),
                look_master: Shadows::new(0.1, 0.4, 0.35),
                look_power: 1.0912,
                gain: 1.0,
            };
            if !gained {
                return agx;
            }
            // The gain that puts mid grey where `tone` puts it: AgX
            // rises through there, so the stops bracket it and halve.
            let want = crate::finish::tone(MID_GREY as f32);
            let (mut lo, mut hi) = (-1.0f32, 3.0f32);
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                if agx.apply([MID_GREY as f32 * 2f32.powf(mid); 3])[1] < want {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            agx.gain = 2f32.powf(0.5 * (lo + hi));
            agx
        }
    }

    /// The white relative exposure in stops over mid grey, the top of
    /// the curve's range.
    #[cfg(test)]
    pub fn white_ev(&self) -> f32 {
        self.log_range - 10.0
    }

    /// The look, Blender's "AgX - Punchy", in the inset space's 25-stop
    /// log: the shadows grade per channel, then as the master, then the
    /// power, which is OCIO's CDL and clamps the log at zero before it
    /// (a floor of about 6e-4 of linear ten stops under mid grey).
    /// Returns the graded color's `log2` per channel and the color
    /// itself, linear, both in the inset space. OCIO runs the look in
    /// its process space and the formation after it reads the result
    /// as a scene color, compensation and inset again, which `apply`
    /// does only where that would change it.
    #[inline]
    fn look_inset(&self, c: [f32; 3]) -> ([f32; 3], [f32; 3]) {
        let c = compensate_low_side(c, LUMA_2020);
        let log2 = apply3(&self.inset, c).map(|v| {
            let x = (v.max(1e-30).log2() - self.look_log_min) / self.look_log_range;
            let x = self.look_master.at(self.look_shadows.at(x));
            x.max(0.0).powf(self.look_power) * self.look_log_range + self.look_log_min
        });
        (log2, log2.map(f32::exp2))
    }

    /// The formation on a linear Rec.2020 color: display-linear
    /// Rec.2020, clipped to the unit cube, as the Rec.2020 formation
    /// LUT holds it.
    #[inline]
    pub fn form(&self, c: [f32; 3]) -> [f32; 3] {
        let c = compensate_low_side(c, LUMA_2020);
        let inset = apply3(&self.inset, c);
        // The log of zero is minus infinity, which the toe takes to
        // zero; held off it so the GPU and the CPU agree there.
        self.form_inset(inset, inset.map(|v| v.max(1e-30).log2()))
    }

    /// The formation from the inset space: the inset color, linear, and
    /// its `log2` per channel. The top is not clipped going in: the
    /// curve keeps rising past one.
    #[inline]
    fn form_inset(&self, inset: [f32; 3], log2: [f32; 3]) -> [f32; 3] {
        let (hue_before, _, _) = hsv(inset);
        let lin = log2.map(|l| {
            let x = ((l - self.log_min) / self.log_range).max(0.0);
            self.curve.at(x).powf(self.gamma)
        });
        let (hue_after, s, v) = hsv(lin);
        let mixed = hsv_to_rgb(lerp_hue(hue_before, hue_after, self.hue_kept), s, v);
        apply3(&self.outset, mixed).map(|v| v.clamp(0.0, 1.0))
    }

    /// The whole transform on a linear working-space color, the gain
    /// first, then the look, then the formation. Where the graded color
    /// taken back out of the inset space has no channel under zero,
    /// the formation's rail and inset would give the inset color back
    /// as it is, so it is handed straight on with its log; otherwise it
    /// goes the long way. Anything not finite is white, as `tone` takes
    /// an infinity to white.
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        if c.iter().any(|v| !v.is_finite()) {
            return [1.0; 3];
        }
        let (log2, inset) = self.look_inset(c.map(|v| v * self.gain));
        let back = apply3(&self.inset_inverse, inset);
        if back.iter().all(|v| *v >= 0.0) {
            self.form_inset(inset, log2)
        } else {
            self.form(back)
        }
    }
}

/// The pipeline's transform; the second construction while the
/// `GREYCARD_AGX_SOFT` variable is set, for the comparison renders of
/// the look call, never by a setting of the editor's.
static BLENDER_PUNCHY: LazyLock<Agx> = LazyLock::new(|| {
    if std::env::var_os("GREYCARD_AGX_SOFT").is_some_and(|v| !v.is_empty()) {
        Agx::blender_punchy_soft()
    } else {
        Agx::blender_punchy()
    }
});

/// The display curve as AgX with the Punchy look, on a linear scene
/// color: display-linear working-space values, where `finish::tone`
/// leaves its. The shader's `tone_agx` is this.
#[inline]
pub fn tone_agx(c: [f32; 3]) -> [f32; 3] {
    BLENDER_PUNCHY.apply(c)
}

/// The output guard rail's weights for an output matrix (working space
/// to the output, rows): the matrix inverted and folded with the
/// luminance weights.
pub fn rail_weights_for(to_out: &Matrix3) -> [f32; 3] {
    let from_out = greycard_core::color::invert3(*to_out).expect("an output matrix inverts");
    rail_weights(&from_out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finish::encode;

    const FIXTURE: &str = include_str!("../tests/fixtures/agx-oracle.txt");
    const MID: f32 = 0.18;

    fn hsv_to_rgb_ring(k: usize) -> [f32; 3] {
        hsv_to_rgb(k as f32 / 12.0, 0.9, 1.0)
    }

    fn color(name: &str) -> [f32; 3] {
        match name {
            "neutral" => [1.0, 1.0, 1.0],
            "red" => [1.0, 0.0, 0.0],
            "green" => [0.0, 1.0, 0.0],
            "blue" => [0.0, 0.0, 1.0],
            "cyan" => [0.0, 1.0, 1.0],
            "magenta" => [1.0, 0.0, 1.0],
            "yellow" => [1.0, 1.0, 0.0],
            "skin" => [0.6, 0.4, 0.3],
            "sky" => [0.3, 0.5, 0.9],
            "foliage" => [0.2, 0.5, 0.1],
            ring => hsv_to_rgb_ring(ring.trim_start_matches("ring").parse().unwrap()),
        }
    }

    fn group(name: &str) -> &'static str {
        match name {
            "neutral" => "neutral",
            "red" | "green" | "blue" | "cyan" | "magenta" | "yellow" => "primaries and secondaries",
            "skin" | "sky" | "foliage" => "skin, sky, foliage",
            _ => "ring outside sRGB",
        }
    }

    /// Blender's own transform, white at 6.5 and no gain, which the
    /// fixture's chain is.
    static BLENDER: LazyLock<Agx> = LazyLock::new(Agx::blender);

    fn form_only(agx: &Agx, c: [f32; 3]) -> [f32; 3] {
        agx.apply(c)
    }

    /// Our sRGB output as the export writes it, the rail at the output
    /// as the finish applies it: 8-bit levels.
    fn ours_srgb(agx: &Agx, c: [f32; 3]) -> [f32; 3] {
        let to_out = crate::export::Space::Srgb.matrix();
        let w = rail_weights_for(&to_out);
        output_rail(apply3(&to_out, form_only(agx, c)), w)
            .map(|v| 255.0 * encode(v.clamp(0.0, 1.0)))
    }

    /// The formation is the LUT's: at 200 grid nodes of Blender's
    /// Rec.2020 formation LUT inside Rec.2020 and within the curve's
    /// range, our formation (no look, no gain) 2.4-encoded is the
    /// LUT's value to a float's rounding, which at a node is the
    /// generator's own arithmetic and not an interpolation of it.
    #[test]
    fn the_formation_is_blenders_at_the_luts_nodes() {
        let agx = &*BLENDER;
        let mut n = 0;
        let mut worst = 0.0f32;
        for line in FIXTURE.lines().filter(|l| l.starts_with("node ")) {
            let v: Vec<f32> = line
                .split_whitespace()
                .skip(1)
                .map(|s| s.parse().unwrap())
                .collect();
            let got = agx
                .form([v[0], v[1], v[2]])
                .map(|x| x.powf(1.0 / agx.gamma));
            for k in 0..3 {
                worst = worst.max((got[k] - v[3 + k]).abs());
            }
            n += 1;
        }
        assert_eq!(n, 200);
        assert!(worst < 2e-6, "{worst}");
    }

    /// Our AgX Punchy against OpenColorIO's render of Blender's config
    /// on the sRGB display (the view "AgX" with the look "AgX - Punchy")
    /// over the fixture's sweep, and the base against the view without
    /// the look, in 8-bit levels; and the Rec.2020 display with the
    /// look, in its own 2.4 encoding. Between its nodes OCIO
    /// interpolates the 57-cubed LUT tetrahedrally; where the formation
    /// has a corner, the clip after the outset on a saturated color and
    /// the sRGB rail, that interpolation lifts a channel at zero by
    /// whole levels, so the bounds are the LUT's measured error and not
    /// the port's: `the_formation_is_blenders_at_the_luts_nodes` holds
    /// the port where the LUT is exact.
    #[test]
    fn agx_punchy_is_opencolorios() {
        let agx = &*BLENDER;
        let mut by_group: std::collections::BTreeMap<(&str, &str), Vec<f32>> = Default::default();
        for line in FIXTURE.lines().filter(|l| l.starts_with("view ")) {
            let f: Vec<&str> = line.split_whitespace().collect();
            let stops: f32 = f[2].parse().unwrap();
            let v: Vec<f32> = f[3..].iter().map(|s| s.parse().unwrap()).collect();
            let input = color(f[1]).map(|c| c * MID * 2f32.powf(stops));
            let g = group(f[1]);
            // sRGB, base: the look taken out of our chain.
            let to_out = crate::export::Space::Srgb.matrix();
            let w = rail_weights_for(&to_out);
            let base = output_rail(apply3(&to_out, agx.form(input)), w)
                .map(|x| 255.0 * encode(x.clamp(0.0, 1.0)));
            let punchy = ours_srgb(agx, input);
            let rec2020 = form_only(agx, input).map(|x| 255.0 * x.clamp(0.0, 1.0).powf(1.0 / 2.4));
            for (which, ours, k0) in [
                ("sRGB base", base, 0),
                ("sRGB Punchy", punchy, 3),
                ("Rec.2020 Punchy", rec2020, 6),
            ] {
                let d = (0..3)
                    .map(|k| (ours[k] - 255.0 * v[k0 + k]).abs())
                    .fold(0.0, f32::max);
                by_group.entry((which, g)).or_default().push(d);
            }
        }
        eprintln!("| view | sweep | median | 90th | max |");
        let mut bounds_ok = true;
        for ((which, g), ds) in &mut by_group {
            ds.sort_by(f32::total_cmp);
            let (med, p90, max) = (ds[ds.len() / 2], ds[ds.len() * 9 / 10], ds[ds.len() - 1]);
            eprintln!("| {which} | {g} | {med:.3} | {p90:.3} | {max:.3} |");
            // The LUT's measured error, a little over it: the neutral
            // to a level (measured 0.23), the memory colors to four on
            // Rec.2020 (3.4) and thirteen on sRGB (12.0, the sky at the
            // sRGB rail's corner), the ring to fifteen (13.3, the
            // output rail without Blender's lerp, see the notes) and
            // the primaries, on the clip's corner, to sixty (55.8).
            let bound = match (*which, *g) {
                (_, "neutral") => 1.0,
                ("Rec.2020 Punchy", "skin, sky, foliage") => 4.0,
                (_, "skin, sky, foliage") => 13.0,
                (_, "ring outside sRGB") => 15.0,
                _ => 60.0,
            };
            if max >= bound {
                eprintln!("  over the bound {bound}");
                bounds_ok = false;
            }
        }
        assert!(bounds_ok);
    }

    /// A neutral through AgX rises all the way, is display white from
    /// six and a half stops over mid grey less the gain on, and black
    /// at black; and the base alone puts mid grey at 0.18, Blender's
    /// pivot.
    #[test]
    fn a_neutral_ramp_through_agx_rises_to_white() {
        let agx = &*BLENDER_PUNCHY;
        let mut last = -1.0f32;
        for k in 0..=720 {
            let stops = -10.0 + k as f32 * 0.025;
            let x = MID * 2f32.powf(stops);
            let out = tone_agx([x; 3]);
            let spread = out.iter().fold(0.0f32, |a, &v| a.max(v))
                - out.iter().fold(1.0f32, |a, &v| a.min(v));
            assert!(
                spread < 1e-5,
                "{stops:+.3} stops: a neutral comes out {out:?}"
            );
            assert!(
                out[1] >= last - 1e-7,
                "{stops:+.3} stops falls: {} after {last}",
                out[1]
            );
            last = out[1];
        }
        // Display white where the curve per channel puts it, the
        // sensor's clip plus the baseline (`DISPLAY_WHITE`, 3.27 stops
        // over mid grey): the white relative exposure is found for it.
        // Found again here by bisection, printed, with the white
        // relative exposure and the encoded values of a neutral at the
        // clip, four and five stops over mid grey through the finish's
        // baseline.
        let (mut lo, mut hi) = (0.0f32, 10.0f32);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if tone_agx([MID * 2f32.powf(mid); 3])[1] < 1.0 - 1e-5 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let white_stops = 0.5 * (lo + hi);
        let want = crate::finish::DISPLAY_WHITE.log2() - MID.log2();
        eprintln!(
            "display white at {white_stops:.3} stops over scene mid grey (per channel's {want:.3}); \
             white relative exposure {:.4} stops",
            agx.white_ev()
        );
        assert!(
            (white_stops - want).abs() < 0.01,
            "{white_stops} against {want}"
        );
        for stops in [2.47f32, 4.0, 5.0] {
            let x = MID * 2f32.powf(stops + crate::finish::BASELINE_EXPOSURE);
            let e = tone_agx([x; 3])[1];
            eprintln!(
                "a neutral {stops:+.2} stops over mid grey, baseline in: display-linear {e:.4}, \
                 {:.1} of 255",
                255.0 * encode(e)
            );
        }
        let white = MID * 2f32.powf(white_stops);
        assert_eq!(tone_agx([white * 4.0; 3]), [1.0; 3]);
        assert!(tone_agx([white * 2f32.powf(-0.3); 3])[1] < 0.995);
        let black = tone_agx([0.0; 3]);
        assert!(black.iter().all(|v| v.abs() < 1e-6), "{black:?}");
        let grey = BLENDER.form([MID; 3]);
        assert!(
            grey.iter().all(|v| (v - MID).abs() < 1e-4),
            "mid grey through Blender's base: {grey:?}"
        );
        assert_eq!(tone_agx([f32::NAN, 0.1, 0.1]), [1.0; 3]);
        assert_eq!(tone_agx([f32::INFINITY; 3]), [1.0; 3]);
    }

    /// Mid grey through the switch in AgX mode is where the curve per
    /// channel puts it, within a thousandth of a display stop, and the
    /// shader's literal gain is the CPU's to a millionth.
    #[test]
    fn mid_grey_through_agx_is_per_channels() {
        use greycard_edit::DisplayCurve;
        let grey = [MID; 3];
        let agx = crate::finish::Source::Scene.curve(grey, DisplayCurve::Agx);
        let channels = crate::finish::Source::Scene.curve(grey, DisplayCurve::Channels);
        for k in 0..3 {
            let stops = (agx[k] / channels[k]).log2().abs();
            assert!(stops < 1e-3, "{agx:?} against {channels:?}");
        }
        let agx = &*BLENDER_PUNCHY;
        eprintln!("the gain is {} stops, {}", agx.gain.log2(), agx.gain);
        let soft = Agx::blender_punchy_soft();
        eprintln!(
            "the second construction: slope {:.4}, white relative exposure {:.4}, gain {} stops, {}",
            soft.curve.slope,
            soft.white_ev(),
            soft.gain.log2(),
            soft.gain
        );
        for stops in [2.47f32, 4.0, 5.0] {
            let x = MID * 2f32.powf(stops + crate::finish::BASELINE_EXPOSURE);
            eprintln!(
                "  a neutral {stops:+.2} stops over mid grey through it: {:.1} of 255",
                255.0 * encode(soft.apply([x; 3])[1])
            );
        }
        // The shader's literals are the CPU's: the gain, the range and
        // the pivot the white sets, and the two scales that follow.
        let shader = include_str!("viewport.wgsl");
        let wanted = [
            ("AGX_GAIN", agx.gain),
            ("AGX_LOG_RANGE", agx.log_range),
            ("AGX_PIVOT_X", agx.curve.pivot_x),
            ("AGX_TOE_SCALE", agx.curve.toe_scale),
            ("AGX_SHOULDER_SCALE", agx.curve.shoulder_scale),
        ];
        let literals: Vec<f32> = wanted
            .iter()
            .map(|(name, value)| {
                let prefix = format!("const {name}: f32 = ");
                let line = shader.lines().find(|l| l.starts_with(&prefix)).expect(name);
                let literal: f32 = line
                    .trim_start_matches(&prefix)
                    .trim_end_matches(';')
                    .parse()
                    .unwrap();
                eprintln!("{name}: cpu {value:.8}, shader {literal:.8}");
                literal
            })
            .collect();
        for ((name, value), literal) in wanted.iter().zip(literals) {
            assert!(
                (literal - value).abs() <= 2e-6 * value.abs().max(1.0),
                "shader {name} {literal} against {value}"
            );
        }
    }

    /// The sRGB formation's departure, measured and pinned: Blender's
    /// sRGB LUT converts the formation to BT.709 unclipped, rails it
    /// there and clips after; our chain clips the formation to the
    /// Rec.2020 cube first, for the point curves and the look table
    /// between the curve and the output. At the sRGB LUT's own grid
    /// nodes (no interpolation), in 8-bit sRGB levels against the
    /// node's value decoded and re-encoded for the file: the nodes
    /// inside Rec.2020 and the curve's range, and the nodes outside
    /// Rec.2020 (where the lower rail acts too). The share over a
    /// level and the largest are printed and held to what was
    /// measured, with a margin. Two things are in these numbers: the
    /// early clip (the tail's part (4) is where it goes) and the lerp
    /// Blender's rail has and darktable's does not; at the full node
    /// grid the clip alone is 2.4 percent of inside nodes over a level
    /// and 2.1 at the 99th percentile, the missing lerp takes that to
    /// 29.3 percent and 17.7 (the notes have the whole table).
    #[test]
    fn the_srgb_formation_at_the_luts_nodes() {
        let agx = &*BLENDER;
        let to_out = crate::export::Space::Srgb.matrix();
        let w = rail_weights_for(&to_out);
        let mut inside: Vec<f32> = Vec::new();
        let mut outside: Vec<f32> = Vec::new();
        for line in FIXTURE.lines().filter(|l| l.starts_with("srgb-node ")) {
            let v: Vec<f32> = line
                .split_whitespace()
                .skip(1)
                .map(|s| s.parse().unwrap())
                .collect();
            let input = [v[0], v[1], v[2]];
            let ours = output_rail(apply3(&to_out, agx.form(input)), w)
                .map(|x| 255.0 * encode(x.clamp(0.0, 1.0)));
            let theirs = [3, 4, 5].map(|k| 255.0 * encode(v[k].powf(2.4)));
            let d = (0..3)
                .map(|k| (ours[k] - theirs[k]).abs())
                .fold(0.0, f32::max);
            if input.iter().all(|x| *x >= 0.0) {
                inside.push(d)
            } else {
                outside.push(d)
            }
        }
        for (label, ds, share_bound, max_bound) in [
            ("inside Rec.2020", &mut inside, 0.4, 25.0),
            ("outside Rec.2020", &mut outside, 0.5, 30.0),
        ] {
            ds.sort_by(f32::total_cmp);
            let over = ds.iter().filter(|d| **d > 1.0).count() as f32 / ds.len() as f32;
            eprintln!(
                "sRGB LUT nodes {label}: {} nodes, median {:.3}, over a level {:.1}%, largest {:.1}",
                ds.len(),
                ds[ds.len() / 2],
                100.0 * over,
                ds[ds.len() - 1]
            );
            assert!(ds.len() >= 100);
            assert!(over < share_bound, "{label}: {over} over a level");
            assert!(
                ds[ds.len() - 1] < max_bound,
                "{label}: {}",
                ds[ds.len() - 1]
            );
        }
    }

    /// The guard rails leave a color inside their gamut exactly alone,
    /// and take one outside to the gamut's face with its luminance
    /// held: the lower rail in Rec.2020, the output rail in sRGB.
    #[test]
    fn the_rails_touch_only_what_is_outside() {
        for c in [
            [0.2f32, 0.5, 0.7],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.01, 0.0, 0.0],
        ] {
            assert_eq!(compensate_low_side(c, LUMA_2020), c);
            assert_eq!(output_rail(c, LUMA_2020), c);
        }
        let dot = |w: [f32; 3], v: [f32; 3]| w[0] * v[0] + w[1] * v[1] + w[2] * v[2];
        let c = [-0.05f32, 0.1, 0.6];
        let out = compensate_low_side(c, LUMA_2020);
        assert_eq!(out[0], 0.0);
        assert!(out[1] > 0.0 && out[2] > 0.0);
        // The luminance kept is the compensated one, which for a color
        // with a negative channel is a little over the plain luminance.
        assert!(dot(LUMA_2020, out) >= dot(LUMA_2020, c) - 1e-6);
        let to_out = crate::export::Space::Srgb.matrix();
        let w = rail_weights_for(&to_out);
        let srgb = apply3(&to_out, [0.0, 0.0, 0.5]);
        assert!(
            srgb.iter().any(|v| *v < 0.0),
            "a Rec.2020 blue is outside sRGB"
        );
        let railed = output_rail(srgb, w);
        assert!(railed.iter().all(|v| *v >= 0.0));
        assert!((railed[0].min(railed[1]).min(railed[2])) == 0.0);
        // Rec.2020's own weights come back for its own matrix.
        let w2020 = rail_weights_for(&crate::export::Space::Rec2020.matrix());
        for k in 0..3 {
            assert!((w2020[k] - LUMA_2020[k]).abs() < 1e-6);
        }
    }

    /// Nanoseconds a pixel, single-threaded, over the test colors scaled
    /// across the range; printed, for the notes.
    #[test]
    #[ignore = "a timing, run by hand in release"]
    fn the_cost_of_a_pixel() {
        let names = [
            "neutral", "red", "green", "blue", "cyan", "magenta", "yellow", "skin", "sky",
            "foliage",
        ];
        let n = 4_000_000usize;
        let px: Vec<[f32; 3]> = (0..n)
            .map(|i| {
                color(names[i % 10])
                    .map(|v| v * MID * 2f32.powf(-8.0 + (i % 977) as f32 / 977.0 * 14.0))
            })
            .collect();
        let _ = tone_agx(px[0]);
        let start = std::time::Instant::now();
        let mut acc = 0.0f32;
        for p in &px {
            acc += tone_agx(*p)[1];
        }
        let agx = start.elapsed().as_nanos() as f64 / n as f64;
        let start = std::time::Instant::now();
        for p in &px {
            acc += p.map(crate::finish::tone)[1];
        }
        let channels = start.elapsed().as_nanos() as f64 / n as f64;
        eprintln!("AgX Punchy {agx:.1} ns a pixel; per channel {channels:.1} ({acc})");
    }
}
