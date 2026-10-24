//! Auto: the Light section's exposure and its four tone sliders set
//! from the frame's own statistics, as a starting point the user then
//! drags from.
//!
//! It reads a luminance histogram of the developed scene before any
//! exposure, the display curve or the tone shifts (linear Rec.2020 Y,
//! in stops over mid grey) and sets Exposure, Highlights, Shadows,
//! Whites and Blacks. Nothing else: not the contrast, the color or the
//! white balance. The same histogram gives the same numbers, to the
//! bit, so a second press changes nothing.
//!
//! The finish's arithmetic it aims through (where display white sits,
//! how far the whites' power reaches, what the black point crushes)
//! is `greycard-render`'s `finish.rs`; the few numbers it needs from
//! there are copied here as constants, and a test there holds them to
//! what they name.

use crate::Light;

/// Mid grey, the stops' zero.
pub const MID_GREY: f32 = 0.18;
/// Rec.2020 luminance weights: the working space's Y.
pub const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// The histogram's bins a stop, each centered on its sixteenth.
const PER_STOP: f32 = 16.0;
/// The histogram's floor and top, in stops over mid grey: under the
/// floor is under any sensor's noise, and over the top is past any
/// clip the develop leaves.
const FLOOR: f32 = -16.0;
const TOP: f32 = 8.0;
const BINS: usize = ((TOP - FLOOR) * PER_STOP) as usize + 1;

/// Scene white, in stops over mid grey before the baseline: where the
/// display curve reaches one once the frame's baseline is on it
/// (`SCENE_WHITE_STOPS` in the finish).
pub const SCENE_WHITE: f32 = 2.47;

/// Where the middle of the frame is brought, in stops over mid grey
/// of the scene before the baseline: the trimmed mean of log
/// luminance between [`MIDDLE_TRIM`]. The baseline goes on top as it
/// does for the default develop, so a frame whose middle sits here
/// already is left at Exposure zero.
pub const MIDDLE: f32 = -1.5;
/// The quantiles the middle is the mean of the log luminance between.
const MIDDLE_TRIM: (f32, f32) = (0.10, 0.90);
/// How far under [`MIDDLE`] a cut may take the frame's shadows, its
/// 25th percentile ([`CUT_AT`]).
const CUT_SHADOWS: f32 = 1.0;
const CUT_AT: f32 = 0.25;
/// Where Whites is held from when the default clips nothing: short of
/// the very top, which a sample places by chance.
const WHITES_HELD: f32 = 0.9995;
/// The lift taken whole; past it, half of the rest.
const FULL_LIFT: f32 = 1.0;
/// A frame lit at night: its 99th percentile within `NIGHT_LIGHTS`
/// stops of white and its median more than `NIGHT_MEDIAN` under
/// [`MIDDLE`]. Its lift is held to `NIGHT_LIFT`.
const NIGHT_LIGHTS: f32 = 1.0;
const NIGHT_MEDIAN: f32 = 2.5;
const NIGHT_LIFT: f32 = 1.0;
/// How far Auto moves the exposure either way, in stops.
const EXPOSURE_LIMIT: f32 = 2.0;
/// The lift cap: a lift never takes the 90th percentile over this
/// many stops under display white, where the display curve gives 0.8
/// of white encoded. A night frame's lamps and lit faces keep their
/// place under white, so its middle is not brought up to day.
pub const P90_UNDER_WHITE: f32 = 1.95;

/// Where the 97th percentile is wanted at most, in stops under display
/// white, and how much of the excess the Highlights slider takes.
const HIGHLIGHTS_AT: (f32, f32) = (0.97, 0.75);
const HIGHLIGHTS_PULL: f32 = 0.5;
/// Where the 3rd percentile is wanted at least, in stops over mid grey
/// after the exposure, and how much of the shortfall Shadows lifts.
const SHADOWS_AT: (f32, f32) = (0.03, -5.0);
const SHADOWS_PULL: f32 = 0.25;
/// Half the Highlights and Shadows sliders' ±2.
const TONE_LIMIT: f32 = 1.0;
/// The share of Highlights the frame's top takes (`highlights_weight`
/// past the ease: the peak, 0.42, eased back by half).
pub const HIGHLIGHTS_AT_TOP: f32 = 0.21;
/// The share of Highlights the bright mid-tones take, at the ramp's
/// peak (`HIGHLIGHTS_PEAK` in the finish).
pub const HIGHLIGHTS_PEAK: f32 = 0.42;

/// The quantiles the clipping pair aims: the 99.5th just under
/// display white, by [`WHITE_MARGIN`] stops, and the 0.5th just over
/// black, at [`BLACK_KEEP`] of its own light.
const WHITES_AT: f32 = 0.995;
const BLACKS_AT: f32 = 0.005;
const WHITE_MARGIN: f32 = 0.1;
const BLACK_KEEP: f32 = 0.5;
/// The Whites slider's ends, and the black point's crush.
const WHITES_LIMIT: f32 = 2.0;
const BLACKS_LIMIT: f32 = 0.3;
/// The whites' pivot (`WHITES_PIVOT` in the finish) and the span its
/// foot hangs from the top by (`HIGHLIGHTS_SPAN`).
pub const WHITES_PIVOT: (f32, f32) = (-2.5, 0.0);
pub const SPAN: (f32, f32) = (1.6, 3.5);
/// The range, in stops from the 0.5th to the 99.5th percentile, over
/// which the pair comes in from nothing to whole when it stretches:
/// a grey card has no ends to put at white and black.
const STRETCH_RANGE: (f32, f32) = (1.0, 3.0);

/// A luminance histogram of the scene before any exposure, in
/// [`PER_STOP`] bins a stop over mid grey, with what sits at or under
/// zero (a pixel the black level took to nothing) counted apart; and
/// beside it the same of each pixel's brightest channel, which is
/// what reaches white first through the display curve.
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    bins: Vec<u64>,
    /// Pixels with no light: under the floor's bin, apart.
    dark: u64,
    total: u64,
    /// The brightest channel's bins, and its pixels with no light.
    peak: Vec<u64>,
    peak_dark: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            bins: vec![0; BINS],
            dark: 0,
            total: 0,
            peak: vec![0; BINS],
            peak_dark: 0,
        }
    }
}

/// The bin of a light `y`, or None for no light.
fn bin(y: f32) -> Option<usize> {
    (y > 0.0).then(|| {
        let s = (y / MID_GREY).log2();
        ((s - FLOOR) * PER_STOP)
            .round()
            .clamp(0.0, (BINS - 1) as f32) as usize
    })
}

/// The share of `total` at or over `stops` in `bins`, counting a bin
/// by the part of it over, as if its pixels were spread evenly
/// through it.
fn share_over(bins: &[u64], total: u64, stops: f32) -> f32 {
    let at = (stops - FLOOR) * PER_STOP;
    let mut over = 0.0f64;
    for (i, &n) in bins.iter().enumerate() {
        let part = (i as f32 + 0.5 - at).clamp(0.0, 1.0);
        over += f64::from(part) * n as f64;
    }
    (over / total.max(1) as f64) as f32
}

/// The quantile `q` of `bins` over `dark` pixels with no light, in
/// stops over mid grey, interpolated within its bin; the floor for one
/// among the pixels with no light, and for an empty histogram.
fn quantile(bins: &[u64], dark: u64, total: u64, q: f32) -> f32 {
    let want = f64::from(q.clamp(0.0, 1.0)) * total as f64;
    let mut below = dark as f64;
    if total == 0 || want <= below {
        return FLOOR;
    }
    for (i, &n) in bins.iter().enumerate() {
        let n = n as f64;
        if n > 0.0 && below + n >= want {
            let f = ((want - below) / n) as f32;
            return FLOOR + (i as f32 - 0.5 + f) / PER_STOP;
        }
        below += n;
    }
    TOP
}

impl Histogram {
    /// The histogram of working-space pixels.
    pub fn of_pixels(pixels: &[[f32; 3]]) -> Self {
        let mut h = Self::default();
        for p in pixels {
            let y = LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2];
            h.add_with(y, p[0].max(p[1]).max(p[2]), 1);
        }
        h
    }

    /// One more pixel of luminance `y`. A value that is not a number
    /// is left out.
    pub fn add(&mut self, y: f32) {
        self.add_n(y, 1);
    }

    /// `n` grey pixels of luminance `y`.
    pub fn add_n(&mut self, y: f32, n: u64) {
        self.add_with(y, y, n);
    }

    /// `n` pixels of luminance `y` and brightest channel `peak`.
    fn add_with(&mut self, y: f32, peak: f32, n: u64) {
        if y.is_nan() || peak.is_nan() {
            return;
        }
        self.total += n;
        match bin(y) {
            Some(i) => self.bins[i] += n,
            None => self.dark += n,
        }
        match bin(peak) {
            Some(i) => self.peak[i] += n,
            None => self.peak_dark += n,
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// The share of pixels with no light.
    fn dark_share(&self) -> f32 {
        self.dark as f32 / self.total.max(1) as f32
    }

    /// The share whose luminance is at or over `stops`.
    #[cfg(test)]
    fn share_over(&self, stops: f32) -> f32 {
        share_over(&self.bins, self.total, stops)
    }

    /// The share whose brightest channel is at or over `stops`.
    fn peak_over(&self, stops: f32) -> f32 {
        share_over(&self.peak, self.total, stops)
    }

    /// The luminance's quantile `q`, in stops over mid grey.
    pub fn quantile(&self, q: f32) -> f32 {
        quantile(&self.bins, self.dark, self.total, q)
    }

    /// The brightest channel's quantile `q`.
    fn peak_quantile(&self, q: f32) -> f32 {
        quantile(&self.peak, self.peak_dark, self.total, q)
    }

    /// `other`'s pixels added to these.
    #[cfg(test)]
    fn merge(&mut self, other: &Histogram) {
        for (a, b) in self.bins.iter_mut().zip(&other.bins) {
            *a += b;
        }
        for (a, b) in self.peak.iter_mut().zip(&other.peak) {
            *a += b;
        }
        self.dark += other.dark;
        self.peak_dark += other.peak_dark;
        self.total += other.total;
    }

    /// The mean of the log luminance over the pixels between the
    /// quantiles `lo` and `hi`, a bin's pixels at its middle and the
    /// bins at the two ends counted by the part of them inside.
    pub fn trimmed_mean(&self, lo: f32, hi: f32) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        let (a, b) = (
            f64::from(lo) * self.total as f64,
            f64::from(hi) * self.total as f64,
        );
        let mut below = 0.0f64;
        let (mut sum, mut count) = (0.0f64, 0.0f64);
        let mut take = |n: f64, at: f32, below: f64| {
            let inside = ((below + n).min(b) - below.max(a)).max(0.0);
            sum += inside * f64::from(at);
            count += inside;
        };
        take(self.dark as f64, FLOOR, below);
        below += self.dark as f64;
        for (i, &n) in self.bins.iter().enumerate() {
            let n = n as f64;
            take(n, FLOOR + i as f32 / PER_STOP, below);
            below += n;
        }
        if count > 0.0 {
            (sum / count) as f32
        } else {
            0.0
        }
    }
}

/// What Auto sets: the Light section's exposure and its four tone
/// sliders, in the edit's own units.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LightChanges {
    pub exposure: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
}

impl LightChanges {
    /// `light` with these in place of its five, the rest (its switch,
    /// the contrast, the baseline) as they were.
    pub fn applied(&self, light: &Light) -> Light {
        let mut l = *light;
        l.exposure = self.exposure;
        l.tone.highlights = self.highlights;
        l.tone.shadows = self.shadows;
        l.tone.whites = self.whites;
        l.tone.blacks = self.blacks;
        l
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Rounded to a hundredth, as a slider shows it, so that what Auto
/// sets is what the panel reads back.
fn hundredth(v: f32) -> f32 {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 { 0.0 } else { r }
}

/// The five sliders for a frame whose scene before exposure has the
/// histogram `h`, at a baseline of `baseline` stops (the body's, or
/// the one an older edit keeps). An empty histogram gives all zeros.
///
/// In order, each on the scene the ones before it leave:
/// - Exposure brings the middle ([`MIDDLE_TRIM`]'s trimmed mean of
///   the log) toward [`MIDDLE`]: a lift whole to a stop and half past
///   it, held to ±2 stops; a frame lit at night ([`NIGHT_MEDIAN`])
///   lifted a stop at most; and any lift held so the 90th percentile
///   stays [`P90_UNDER_WHITE`] under display white. A cut never takes
///   the 25th percentile more than [`CUT_SHADOWS`] under the target;
///   the cut refused goes to Highlights, and only what the top asks
///   past Highlights' limit comes off the exposure.
/// - Highlights pulls by half of how far the 97th percentile sits over
///   three quarters of a stop under white, Shadows lifts by a quarter
///   of how far the 3rd sits under five stops under grey; each
///   only that way, and to half its slider.
/// - Whites puts the 99.5th percentile a tenth of a stop under white,
///   Blacks the 0.5th at half its light; a stretch (Whites up, Blacks
///   crushing) comes in with the frame's range over
///   [`STRETCH_RANGE`]. Neither ever clips more than the default edit
///   does: Whites is held so no more of the frame reaches white than
///   at the default, after the exposure, and Blacks so no more goes to
///   black.
pub fn auto_light(h: &Histogram, baseline: f32) -> LightChanges {
    if h.total() == 0 {
        return LightChanges::default();
    }
    // All in stops over mid grey of the scene before the baseline,
    // where display white is `SCENE_WHITE` whatever the body.
    let middle = h.trimmed_mean(MIDDLE_TRIM.0, MIDDLE_TRIM.1);
    // The key kept: a lift is whole up to `FULL_LIFT` and half past
    // it, so a dark frame ends darker than an ordinary one.
    let short = MIDDLE - middle;
    let mut exposure = if short > FULL_LIFT {
        FULL_LIFT + (short - FULL_LIFT) / 2.0
    } else {
        short
    }
    .clamp(-EXPOSURE_LIMIT, EXPOSURE_LIMIT);
    // A frame lit at night: real lights near white and a median far
    // under the target. Its median rises a stop at most.
    let median = h.quantile(0.5);
    if h.quantile(0.99) >= SCENE_WHITE - NIGHT_LIGHTS && median < MIDDLE - NIGHT_MEDIAN {
        exposure = exposure.min(NIGHT_LIFT);
    }
    if exposure > 0.0 {
        let room = SCENE_WHITE - P90_UNDER_WHITE - h.quantile(0.90);
        exposure = exposure.min(room.max(0.0));
    }
    // A cut never takes the shadows more than `CUT_SHADOWS` under the
    // target: a backlit frame's middle is its sky, and its subject is
    // its dark part. The cut refused is Highlights' job, as a hand
    // edit would take the sky down; only what the top asks past
    // Highlights' limit comes off the exposure, never past the cut
    // first asked.
    let asked = exposure;
    if exposure < 0.0 {
        exposure = exposure.max((MIDDLE - CUT_SHADOWS - h.quantile(CUT_AT)).min(0.0));
        let over = h.quantile(HIGHLIGHTS_AT.0) + exposure - (SCENE_WHITE - HIGHLIGHTS_AT.1);
        let beyond = over - TONE_LIMIT / HIGHLIGHTS_PULL;
        if beyond > 0.0 {
            exposure = (exposure - beyond).max(asked);
        }
    }
    // The cut refused, before the rounding, so a lift or a cut taken
    // whole hands Highlights nothing.
    let refused = (exposure - asked.min(0.0)).max(0.0).min(-asked.min(0.0));
    let mut exposure = hundredth(exposure);

    let p97 = h.quantile(HIGHLIGHTS_AT.0) + exposure;
    let highlights = (-(p97 - (SCENE_WHITE - HIGHLIGHTS_AT.1)).max(0.0) * HIGHLIGHTS_PULL)
        .min(-refused / HIGHLIGHTS_PEAK)
        .max(-TONE_LIMIT);
    let p3 = h.quantile(SHADOWS_AT.0) + exposure + baseline;
    let shadows = ((SHADOWS_AT.1 - p3).max(0.0) * SHADOWS_PULL).min(TONE_LIMIT);

    let (lo, hi) = (h.quantile(BLACKS_AT), h.quantile(WHITES_AT));
    let stretch = smoothstep(STRETCH_RANGE.0, STRETCH_RANGE.1, hi - lo);

    // Whites, in the finish's terms: display white `SCENE_WHITE` over
    // grey, a pivot `f` at the foot of the frame's span, and a power
    // that takes `top - f` to `(top - f) * reach / (reach - w)`.
    let top = hi + exposure + highlights * HIGHLIGHTS_AT_TOP;
    let median = h.quantile(0.5) + exposure;
    let p99 = h.quantile(0.99) + exposure;
    let f = (p99 - (p99 - median).clamp(SPAN.0, SPAN.1)).clamp(WHITES_PIVOT.0, WHITES_PIVOT.1);
    let reach = SCENE_WHITE - f;
    let mut whites = if top - f > 0.5 {
        reach - (top - f) * reach / (reach - WHITE_MARGIN)
    } else {
        0.0
    };
    if whites > 0.0 {
        whites *= stretch;
    }
    // Never more at white than the default: the white point takes
    // `SCENE_WHITE - w` to white, so `w` is held to put that no lower
    // than where the default's own clipped share starts, after the
    // exposure. The highlights' pull is left out, which only clips
    // less.
    // Where the default clips nothing the bound is the 99.95th
    // percentile and a margin, not the brightest point: the grid Auto
    // reads is a sample, and its brightest point moves with where the
    // grid falls.
    // Read on each pixel's brightest channel, since a channel reaches
    // white before the luminance does: the white point's gain rises
    // with the luminance, so holding the channel holds the pixel.
    let clipped = h.peak_over(SCENE_WHITE);
    let start = if clipped > 0.0 {
        h.peak_quantile(1.0 - clipped)
    } else {
        h.peak_quantile(WHITES_HELD) + WHITE_MARGIN
    };
    // Rounded down to the hundredth the slider shows, which only
    // clips less.
    whites = (whites.min(SCENE_WHITE - exposure - start) * 100.0).floor() / 100.0;
    // And a frame the default clips is never stretched toward it.
    if clipped > 0.0 {
        whites = whites.min(0.0);
    }
    // A frame so far over white that Whites cannot hold it takes the
    // rest off the exposure.
    if whites < -WHITES_LIMIT {
        exposure =
            (((exposure + WHITES_LIMIT + whites) * 100.0).floor() / 100.0).max(-EXPOSURE_LIMIT);
        whites = -WHITES_LIMIT;
    }
    let whites = whites.min(WHITES_LIMIT);

    // Blacks crushes `-blacks` of mid grey to nothing, after the
    // baseline and the exposure. Never more at black than the default:
    // no deeper than the darkest light the default leaves.
    let gain = 2f32.powf(baseline + exposure);
    let at = MID_GREY * 2f32.powf(lo) * gain;
    let floor = MID_GREY * 2f32.powf(h.quantile(h.dark_share())) * gain;
    let crush = (at * BLACK_KEEP).min(floor) / MID_GREY * stretch;
    // Rounded toward none, which only crushes less.
    let blacks = -((crush.min(BLACKS_LIMIT) * 100.0).floor() / 100.0);

    LightChanges {
        exposure,
        highlights: hundredth(highlights),
        shadows: hundredth(shadows),
        whites: hundredth(whites),
        blacks: hundredth(blacks),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A histogram of `n` pixels spread evenly in stops from `lo` to
    /// `hi` over mid grey.
    fn spread(lo: f32, hi: f32, n: u32) -> Histogram {
        let mut h = Histogram::default();
        for i in 0..n {
            let s = lo + (hi - lo) * (i as f32 + 0.5) / n as f32;
            h.add(MID_GREY * 2f32.powf(s));
        }
        h
    }

    #[test]
    fn a_frame_a_stop_dark_takes_a_stop() {
        // A frame whose middle sits at the target, and the same a stop
        // under: Auto lifts the second by a stop, within a twentieth.
        let at = auto_light(&spread(MIDDLE - 2.0, MIDDLE + 2.0, 20_000), 0.8);
        let dark = auto_light(&spread(MIDDLE - 3.0, MIDDLE + 1.0, 20_000), 0.8);
        assert!(at.exposure.abs() < 0.05, "{at:?}");
        assert!((dark.exposure - 1.0).abs() < 0.05, "{dark:?}");
    }

    #[test]
    fn a_dark_frame_keeps_some_of_its_key() {
        // Three stops under: a stop whole and half of the other two,
        // so it ends a stop under where an ordinary frame sits.
        let a = auto_light(&spread(MIDDLE - 5.0, MIDDLE - 1.0, 20_000), 0.8);
        assert!((a.exposure - 2.0).abs() < 0.05, "{a:?}");
        let b = auto_light(&spread(MIDDLE - 4.0, MIDDLE, 20_000), 0.8);
        assert!((b.exposure - 1.5).abs() < 0.05, "{b:?}");
    }

    #[test]
    fn a_backlit_frame_keeps_its_shadows() {
        // The backlit couple's shape: a quarter of the frame the
        // subject, its 25th percentile at -2.46, and a sky over the
        // rest that puts the 90th percentile bright. The trimmed mean
        // asks for a cut; the shadows refuse it, and Highlights takes
        // the sky at its limit.
        let mut h = spread(-4.0, -2.46, 2_500);
        h.merge(&spread(-2.46, 2.4, 7_500));
        assert!((h.quantile(CUT_AT) + 2.46).abs() < 0.02);
        assert!(h.quantile(0.9) > 1.5);
        let asked = MIDDLE - h.trimmed_mean(0.1, 0.9);
        assert!(asked < -0.5, "{asked}");
        let a = auto_light(&h, 0.8);
        assert!(a.exposure.abs() <= 0.1, "{a:?}");
        assert_eq!(a.highlights, -TONE_LIMIT, "{a:?}");
    }

    #[test]
    fn a_frame_lit_at_night_rises_a_stop_at_most() {
        // Its 90th percentile dark, so the lift cap does not hold it,
        // but three percent of it lamps near white.
        let mut h = spread(MIDDLE - 7.0, MIDDLE - 3.5, 9_700);
        h.add_n(MID_GREY * 2f32.powf(SCENE_WHITE - 0.3), 300);
        let a = auto_light(&h, 0.8);
        assert!(a.exposure > 0.5 && a.exposure <= 1.0, "{a:?}");
    }

    #[test]
    fn a_clipped_top_takes_whites_down_never_up() {
        // Frames with a tenth of them at the sensor's clip, bright and
        // dark below it: whatever Auto does to the exposure, Whites is
        // never up, and holds the clip where the default had it.
        for (lo, hi) in [(-4.0, 1.5), (-7.0, -2.0)] {
            let mut h = spread(lo, hi, 9_000);
            h.add_n(MID_GREY * 2f32.powf(SCENE_WHITE + 0.01), 1_000);
            let a = auto_light(&h, 0.8);
            assert!(a.whites <= 0.0, "{a:?}");
            let clipped = h.share_over(SCENE_WHITE - a.exposure - a.whites);
            assert!(clipped <= h.share_over(SCENE_WHITE) + 1e-6, "{a:?}");
            // The dark one is lifted, so Whites comes down for it.
            if lo < -5.0 {
                assert!(a.exposure > 0.0 && a.whites < 0.0, "{a:?}");
            }
        }
    }

    #[test]
    fn a_dark_frame_lifted_does_not_clip_what_the_default_did_not() {
        // A stop and a half under with a few bright pixels near the
        // clip: the lift would push them over, so Whites comes down.
        let mut h = spread(-4.5, 0.0, 9_990);
        h.add_n(MID_GREY * 2f32.powf(2.3), 10);
        let a = auto_light(&h, 0.8);
        assert!(a.exposure > 0.5, "{a:?}");
        assert_eq!(h.share_over(SCENE_WHITE), 0.0);
        assert!(
            h.share_over(SCENE_WHITE - a.exposure - a.whites) <= 1e-6,
            "{a:?}"
        );
    }

    #[test]
    fn a_saturated_highlight_is_held_by_its_brightest_channel() {
        // Grey over most of the frame, and a red light whose luminance
        // sits two stops under white but whose red channel is within a
        // tenth of a stop of it: Whites must not stretch it over.
        let mut px: Vec<[f32; 3]> = (0..9_900)
            .map(|i| [MID_GREY * 2f32.powf(-4.0 + 4.5 * i as f32 / 9_900.0); 3])
            .collect();
        let red = MID_GREY * 2f32.powf(SCENE_WHITE - 0.1);
        px.extend(std::iter::repeat_n([red, 0.1 * red, 0.1 * red], 100));
        let h = Histogram::of_pixels(&px);
        let a = auto_light(&h, 0.8);
        let reaches = h.peak_quantile(WHITES_HELD) + a.exposure + a.whites;
        assert!(reaches <= SCENE_WHITE, "{a:?}");
    }

    #[test]
    fn a_flat_grey_frame_is_left_alone() {
        let mut h = Histogram::default();
        h.add_n(MID_GREY * 2f32.powf(MIDDLE), 10_000);
        assert_eq!(auto_light(&h, 0.8), LightChanges::default());
        assert_eq!(
            auto_light(&Histogram::default(), 0.8),
            LightChanges::default()
        );
    }

    #[test]
    fn the_same_frame_gives_the_same_numbers() {
        let mut h = spread(-6.0, 2.0, 30_000);
        h.add_n(0.0, 50);
        let (a, b) = (auto_light(&h, 0.8), auto_light(&h.clone(), 0.8));
        let bits = |c: LightChanges| {
            [c.exposure, c.highlights, c.shadows, c.whites, c.blacks].map(f32::to_bits)
        };
        assert_eq!(bits(a), bits(b));
    }

    #[test]
    fn a_night_frame_is_not_lifted_into_day() {
        // Most of the frame five stops under grey, a seventh of it
        // lamps and lit walls a stop over: the lift stops where the 90th
        // percentile reaches its cap.
        let mut h = spread(-7.0, -4.0, 9_000);
        let lamps = spread(1.0, 1.5, 1_500);
        h.merge(&lamps);
        let a = auto_light(&h, 0.8);
        let room = SCENE_WHITE - P90_UNDER_WHITE - h.quantile(0.9);
        assert!(a.exposure <= room.max(0.0) + 0.01, "{a:?}");
        assert!(a.exposure < 0.5, "{a:?}");
    }

    #[test]
    fn the_tone_pair_moves_one_way_each_and_to_half() {
        // A frame with a bright top and a deep bottom.
        let a = auto_light(&spread(-12.0, 4.0, 30_000), 0.8);
        assert!(a.highlights < 0.0 && a.highlights >= -TONE_LIMIT, "{a:?}");
        assert!(a.shadows > 0.0 && a.shadows <= TONE_LIMIT, "{a:?}");
        // A narrow frame about grey: neither.
        let b = auto_light(&spread(-1.0, 1.0, 30_000), 0.8);
        assert_eq!((b.highlights, b.shadows), (0.0, 0.0), "{b:?}");
    }

    #[test]
    fn quantiles_read_within_a_bin() {
        let h = spread(-2.0, 2.0, 64_000);
        assert!((h.quantile(0.5)).abs() < 0.02);
        assert!((h.quantile(0.25) + 1.0).abs() < 0.02);
        assert!(h.trimmed_mean(0.1, 0.9).abs() < 0.02);
        let mut d = Histogram::default();
        d.add_n(0.0, 10);
        assert_eq!(d.quantile(0.5), FLOOR);
    }

    #[test]
    fn auto_replaces_only_its_five() {
        let mut light = Light::default();
        light.tone.contrast = 1.3;
        light.baseline = Some(0.8);
        light.enabled = false;
        let c = LightChanges {
            exposure: 0.5,
            highlights: -0.3,
            shadows: 0.2,
            whites: 0.1,
            blacks: -0.01,
        };
        let l = c.applied(&light);
        assert_eq!(
            (l.tone.contrast, l.baseline, l.enabled),
            (1.3, Some(0.8), false)
        );
        assert_eq!(
            (
                l.exposure,
                l.tone.highlights,
                l.tone.shadows,
                l.tone.whites,
                l.tone.blacks
            ),
            (0.5, -0.3, 0.2, 0.1, -0.01)
        );
    }
}
