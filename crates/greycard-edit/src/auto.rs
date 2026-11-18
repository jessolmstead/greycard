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
//! A merge of ours keeps range over its reference's clip: stored that
//! many stops under, and brightened by as much in the develop
//! ([`Headroom`]). Auto reads it as the develop shows it, its
//! headroom on, and reads its kept range over display white as
//! Highlights pulls it in the finish, so it lands where its
//! reference's Auto does and leaves the kept range to Highlights.
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
/// Whites down's roll over display white (issue 132), shared by the
/// finish (`whites_roll`), the shader, and Auto's model: the slope over
/// its knee, how soft its two corners are, in stops either side, and the
/// ease its reach comes in with.
pub const WHITES_ROLL_SLOPE: f32 = 0.2;
pub const WHITES_ROLL_SOFT: f32 = 0.1;
pub const WHITES_ROLL_EASE: f32 = 0.1;

/// The stops Whites down's roll takes off a channel `t` stops from
/// display white, after the white point, with the whites `down` stops
/// under zero (held as the finish holds it).
///
/// A light `reach` stops over white lands on it, within a fiftieth of a
/// stop (the rounded corner's), where the reach is
/// `down` eased in ([`whites_roll_reach`]), so it starts with no slope
/// at zero. Under white the
/// roll makes room by a knee `s / (1 - s)` of the reach under white
/// ([`WHITES_ROLL_SLOPE`] `s`), over which the slope is `s` up to the
/// reach; past it the slope is one again and the pull its most, the
/// reach, so a light the roll cannot bring under white stays where the
/// white point put it, less the reach, over white. Both corners are
/// rounded over [`WHITES_ROLL_SOFT`] either side, by a parabola, so the
/// pull and its slope are continuous in the channel and in `down`.
///
/// Zero at `down` zero and for any channel more than the softness under
/// the knee; never more than the reach, so it falls to nothing with
/// `down`; its slope in `t` between zero and `1 - s`, so the roll never
/// turns a light back; and never less as `down` grows: the knee goes
/// down and the reach up.
pub fn whites_roll_pull(t: f32, down: f32) -> f32 {
    if down <= 0.0 {
        return 0.0;
    }
    let (s, w) = (WHITES_ROLL_SLOPE, WHITES_ROLL_SOFT);
    let reach = whites_roll_reach(down);
    let knee = whites_roll_knee(down);
    // A ramp from zero with a slope of one, its corner at `w`, rounded.
    let ramp = |z: f32| {
        if z <= 0.0 {
            0.0
        } else if z < 2.0 * w {
            z * z / (4.0 * w)
        } else {
            z - w
        }
    };
    (1.0 - s) * (ramp(t - knee + w) - ramp(t - reach + w))
}

/// The stops over display white Whites `down` stops under zero brings
/// to white by its roll: `down` eased in, `down^3 / (down^2 + e^2)` with
/// `e` [`WHITES_ROLL_EASE`], which is `down` within a fiftieth of a stop
/// from a half and has neither value nor slope at zero.
pub fn whites_roll_reach(down: f32) -> f32 {
    let e = WHITES_ROLL_EASE;
    down * down * down / (down * down + e * e)
}

/// Where the roll's slope starts, in stops from display white: the
/// room the reach needs at the roll's slope, under white.
pub fn whites_roll_knee(down: f32) -> f32 {
    let s = WHITES_ROLL_SLOPE;
    -s * whites_roll_reach(down) / (1.0 - s)
}

/// Where the whites `w` puts a grey `x` stops over mid grey (after the
/// exposure), in stops from display white: the finish's white point
/// about a pivot `f` (its power past the first stop, where it is exact)
/// and, down, its roll over white ([`whites_roll_pull`]).
pub fn whites_lands(x: f32, w: f32, f: f32) -> f32 {
    let reach = SCENE_WHITE - f;
    let y = if x > f {
        f + (x - f) * reach / (reach - w)
    } else {
        x
    };
    let t = y - SCENE_WHITE;
    t - whites_roll_pull(t, -w)
}

/// The least whites the finish acts on (`WHITES_RANGE` there).
pub const WHITES_FLOOR: f32 = -3.5;

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

/// What a frame keeps over display white: a merge of ours, its
/// samples stored `stops` under its reference's scale and brightened
/// by as much in the develop, and the frame's median and top as the
/// render reads them (the guide plane's `frame_range`), in stops over
/// mid grey of the frame as stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Headroom {
    pub stops: f32,
    pub range: [f32; 2],
}

/// The finish's pull on a merge's kept range ([`crate::kept`]), as
/// Auto reads it: at contrast one, in Auto's stops (the scene with the
/// headroom on and the exposure `exposure`, display white at
/// [`SCENE_WHITE`]). The negative travel of Highlights compresses what
/// reads over a knee [`SPAN`]'s least under white toward it, `share`
/// of the excess at the frame's top, the excess its top's height over
/// white, at most the headroom. `None` where nothing reads over white.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pull {
    top: f32,
    knee: f32,
    excess: f32,
}

impl Pull {
    pub fn of(headroom: Headroom, exposure: f32) -> Option<Pull> {
        if headroom.stops <= 0.0 {
            return None;
        }
        let top = headroom.range[1] + headroom.stops + exposure;
        let excess = (top - SCENE_WHITE).min(headroom.stops);
        (excess > 0.0).then_some(Pull {
            top,
            knee: SCENE_WHITE - SPAN.0,
            excess,
        })
    }

    /// The share of the pull a Highlights of `highlights` asks for.
    pub fn share(highlights: f32) -> f32 {
        crate::kept::share(highlights)
    }

    /// A region reading `g` with `share` of the pull on it.
    pub fn at(&self, g: f32, share: f32) -> f32 {
        crate::kept::pull(g, share, self.knee, self.top, self.excess)
    }

    /// A pixel reading `g` with `share` of the pull on it, as the finish
    /// moves a pixel: by its region's pull. The region is taken to read
    /// `g` up to the frame's top, the top past it, so a pixel over the
    /// top keeps what it reads over its region.
    pub fn at_pixel(&self, g: f32, share: f32) -> f32 {
        let region = g.min(self.top);
        g + self.at(region, share) - region
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
/// the one an older edit keeps): [`auto_light_kept`] for a frame that
/// keeps nothing over display white.
pub fn auto_light(h: &Histogram, baseline: f32) -> LightChanges {
    auto_light_kept(h, baseline, None)
}

/// The five sliders for a frame whose scene before exposure, as
/// stored, has the histogram `h`, at a baseline of `baseline` stops
/// (the exposure the slider's zero stands for: the body's, or the one
/// an older edit keeps, and a merge's headroom on top), with
/// `headroom` what a merge keeps over display white. An empty
/// histogram gives all zeros.
///
/// A merge is read as the develop shows it, its headroom on, so its
/// middle and its top sit where its reference's do. Its kept range is
/// Highlights' to bring down, as the finish pulls it: where Whites
/// reads the frame's top, and where the cut asks what the top wants
/// past Highlights' limit, the top is read through the pull at that
/// Highlights. Highlights itself reads the frame as the default shows
/// it, as on any frame.
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
pub fn auto_light_kept(h: &Histogram, baseline: f32, headroom: Option<Headroom>) -> LightChanges {
    if h.total() == 0 {
        return LightChanges::default();
    }
    // All in stops over mid grey of the scene before the baseline,
    // where display white is `SCENE_WHITE` whatever the body: a merge's
    // read with its headroom on, as the develop shows it.
    let shift = headroom.map_or(0.0, |k| k.stops);
    let body = baseline - shift;
    let q = |at: f32| h.quantile(at) + shift;
    let pull = |exposure: f32| headroom.and_then(|k| Pull::of(k, exposure));
    let pulled = |g: f32, exposure: f32, highlights: f32| {
        pull(exposure).map_or(g, |p| p.at_pixel(g, Pull::share(highlights)))
    };
    let middle = h.trimmed_mean(MIDDLE_TRIM.0, MIDDLE_TRIM.1) + shift;
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
    let median = q(0.5);
    if q(0.99) >= SCENE_WHITE - NIGHT_LIGHTS && median < MIDDLE - NIGHT_MEDIAN {
        exposure = exposure.min(NIGHT_LIFT);
    }
    if exposure > 0.0 {
        let room = SCENE_WHITE - P90_UNDER_WHITE - q(0.90);
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
        exposure = exposure.max((MIDDLE - CUT_SHADOWS - q(CUT_AT)).min(0.0));
        // Highlights' limit pulls a merge's kept range as far as it
        // reaches.
        let p97 = pulled(q(HIGHLIGHTS_AT.0) + exposure, exposure, -TONE_LIMIT);
        let over = p97 - (SCENE_WHITE - HIGHLIGHTS_AT.1);
        let beyond = over - TONE_LIMIT / HIGHLIGHTS_PULL;
        if beyond > 0.0 {
            exposure = (exposure - beyond).max(asked);
        }
    }
    // The cut refused, before the rounding, so a lift or a cut taken
    // whole hands Highlights nothing.
    let refused = (exposure - asked.min(0.0)).max(0.0).min(-asked.min(0.0));
    let mut exposure = hundredth(exposure);

    let p97 = q(HIGHLIGHTS_AT.0) + exposure;
    let highlights = (-(p97 - (SCENE_WHITE - HIGHLIGHTS_AT.1)).max(0.0) * HIGHLIGHTS_PULL)
        .min(-refused / HIGHLIGHTS_PEAK)
        .max(-TONE_LIMIT);
    let p3 = q(SHADOWS_AT.0) + exposure + body;
    let shadows = ((SHADOWS_AT.1 - p3).max(0.0) * SHADOWS_PULL).min(TONE_LIMIT);

    let (lo, hi) = (h.quantile(BLACKS_AT), h.quantile(WHITES_AT));
    let stretch = smoothstep(STRETCH_RANGE.0, STRETCH_RANGE.1, hi - lo);
    let hi = hi + shift;

    // Whites, in the finish's terms: display white `SCENE_WHITE` over
    // grey, a pivot `f` at the foot of the frame's span, and a power
    // that takes `top - f` to `(top - f) * reach / (reach - w)`. A
    // merge's top as Highlights pulls it; the pivot on the span as the
    // frame has it, as the finish keeps it.
    // The white point alone: down, the roll over white after it then
    // takes the top a little further under white (`whites_lands`) and
    // brings the lights over it under white too. Aimed through the roll
    // a tenth under white, the top sat in its band, where a warm
    // subject's brightest channel came within a hundredth of a stop of
    // white and showed flat white; aimed under the band, a merge's Auto
    // took a stop off its exposure.
    let top = pulled(hi + exposure, exposure, highlights) + highlights * HIGHLIGHTS_AT_TOP;
    let median = q(0.5) + exposure;
    let p99 = q(0.99) + exposure;
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
    let clipped = h.peak_over(SCENE_WHITE - shift);
    let start = if clipped > 0.0 {
        h.peak_quantile(1.0 - clipped) + shift
    } else {
        h.peak_quantile(WHITES_HELD) + WHITE_MARGIN + shift
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

    /// Whites down is set by the white point alone, as 0.12 set it: the
    /// top a tenth under white by the power. The roll after it then takes
    /// the top further under white, never over, on frames whose top is
    /// from half a stop to three over white: by 0.02, 0.10, 0.28, and 0.31
    /// stops at Whites -0.45, -0.92, -1.84, and -2, so the top lands 0.12
    /// to 0.42 under white. Aiming through the roll was tried and left a
    /// bright subject in the roll's band at flat white (`auto_light`).
    #[test]
    fn whites_down_lands_the_top_through_the_roll() {
        for (over, lands) in [
            (0.5f32, -0.1248),
            (1.0, -0.2022),
            (2.0, -0.3878),
            (3.0, -0.4221),
        ] {
            let mut h = spread(-5.0, SCENE_WHITE - 0.8, 9_000);
            for i in 0..1_000 {
                let at = SCENE_WHITE - 0.8 + (0.8 + over) * (i as f32 + 0.5) / 1_000.0;
                h.add(MID_GREY * 2f32.powf(at));
            }
            let a = auto_light(&h, 0.8);
            assert!(a.whites < 0.0, "{over}: {a:?}");
            let (median, p99) = (h.quantile(0.5) + a.exposure, h.quantile(0.99) + a.exposure);
            let f =
                (p99 - (p99 - median).clamp(SPAN.0, SPAN.1)).clamp(WHITES_PIVOT.0, WHITES_PIVOT.1);
            let top = h.quantile(WHITES_AT) + a.exposure + a.highlights * HIGHLIGHTS_AT_TOP;
            let alone = top - SCENE_WHITE
                + (top - f) * (SCENE_WHITE - f) / (SCENE_WHITE - f - a.whites)
                - (top - f);
            let rolled = whites_lands(top, a.whites, f);
            assert!(
                alone <= -WHITE_MARGIN + 0.02 && alone > -WHITE_MARGIN - 0.1,
                "{over}: {alone} {a:?}"
            );
            assert!(rolled <= alone, "{over}: {rolled} {alone}");
            assert!(
                (rolled - lands).abs() < 0.003,
                "{over}: {rolled} for {lands}"
            );
        }
    }

    /// The roll: nothing at zero and under its knee's band; the reach
    /// lands on white; the slope in the channel between `s` and one;
    /// never less as the whites goes down; never more than the reach.
    #[test]
    fn the_whites_roll_holds_its_shape() {
        let (s, w) = (WHITES_ROLL_SLOPE, WHITES_ROLL_SOFT);
        for i in -300..=600 {
            assert_eq!(whites_roll_pull(i as f32 * 0.01, 0.0), 0.0);
        }
        // The reach, eased in under a tenth.
        for (down, reach) in [(0.1f32, 0.05), (0.25, 0.2155), (1.0, 0.9901)] {
            let r = whites_roll_reach(down);
            assert!((r - reach).abs() < 1e-4, "{down}: {r}");
        }
        for k in 1..=350 {
            let down = k as f32 * 0.01;
            let reach = whites_roll_reach(down);
            let knee = whites_roll_knee(down);
            assert_eq!(whites_roll_pull(knee - w - 1e-4, down), 0.0, "{down}");
            assert!(whites_roll_pull(knee - w + 1e-3, down) > 0.0, "{down}");
            let mut last = whites_roll_pull(-4.0, down);
            for i in 1..=1000 {
                let t = -4.0 + i as f32 * 0.01;
                let p = whites_roll_pull(t, down);
                let slope = (p - last) / 0.01;
                assert!(
                    slope >= -1e-3 && slope <= 1.0 - s + 1e-3,
                    "{down} {t}: {slope}"
                );
                assert!(p <= reach + 1e-5, "{down} {t}: {p} past {reach}");
                assert!(p >= whites_roll_pull(t, down - 0.01) - 1e-6, "{down} {t}");
                last = p;
            }
            // Under white to the reach's corner, a fiftieth of a stop
            // over it at the reach.
            let landed = |t: f32| t - whites_roll_pull(t, down);
            assert!(landed(reach - w) <= 1e-5, "{down}: {}", landed(reach - w));
            assert!(
                landed(reach) <= (1.0 - s) * w / 4.0 + 1e-5,
                "{down}: {}",
                landed(reach)
            );
        }
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

    /// A frame stored `stops` under, as a merge is, read at its
    /// headroom: its luminances `stops` lower, and its brightest
    /// channels with them.
    fn stored(lo: f32, hi: f32, n: u32, kept: &[(f32, u64)], stops: f32) -> Histogram {
        let mut h = spread(lo - stops, hi - stops, n);
        for &(at, count) in kept {
            h.add_n(MID_GREY * 2f32.powf(at - stops), count);
        }
        h
    }

    /// A merge whose range sits under display white reads as the frame
    /// it was made from: its headroom on, it lands where the same
    /// scene unstored does.
    #[test]
    fn a_merge_under_white_reads_as_its_scene() {
        for (lo, hi) in [(MIDDLE - 3.0, MIDDLE + 2.0), (-7.0, -2.0), (-4.0, 1.5)] {
            let scene = auto_light(&spread(lo, hi, 20_000), 0.8);
            let stops = 2.5;
            let merge = auto_light_kept(
                &stored(lo, hi, 20_000, &[], stops),
                0.8 + stops,
                Some(Headroom {
                    stops,
                    range: [(lo + hi) / 2.0 - stops, hi - stops],
                }),
            );
            for (a, b) in [
                (merge.exposure, scene.exposure),
                (merge.highlights, scene.highlights),
                (merge.shadows, scene.shadows),
                (merge.whites, scene.whites),
                (merge.blacks, scene.blacks),
            ] {
                assert!(
                    (a - b).abs() <= 0.02,
                    "{lo}..{hi}: {merge:?} against {scene:?}"
                );
            }
        }
    }

    /// A merge's kept range is Highlights' to bring down: a frame
    /// three stops over white at its top, read with its headroom on
    /// but its range taken as it stands, sends Whites to its limit and
    /// the rest off the exposure; read through the pull, as the
    /// finish shows it at Auto's Highlights, it leaves the exposure
    /// where the frame's middle asks and Whites short of its limit.
    #[test]
    fn a_merges_kept_range_is_left_to_highlights() {
        let stops = 3.0;
        // The scene to a stop under white, and a tenth of it spread
        // over white to the headroom's top.
        let kept: Vec<(f32, u64)> = (0..100)
            .map(|i| (SCENE_WHITE + stops * (i as f32 + 0.5) / 100.0, 40))
            .collect();
        let h = stored(-7.0, SCENE_WHITE - 1.0, 36_000, &kept, stops);
        let headroom = Headroom {
            stops,
            range: [-2.0 - stops, SCENE_WHITE + 2.7 - stops],
        };
        let mut unpulled = spread(-7.0, SCENE_WHITE - 1.0, 36_000);
        for &(at, count) in &kept {
            unpulled.add_n(MID_GREY * 2f32.powf(at), count);
        }
        let read = auto_light(&unpulled, 0.8);
        assert_eq!(read.whites, -WHITES_LIMIT, "{read:?}");
        let merge = auto_light_kept(&h, 0.8 + stops, Some(headroom));
        assert!(merge.whites > -WHITES_LIMIT + 0.3, "{merge:?}");
        assert!(
            merge.exposure > read.exposure + 0.3,
            "{merge:?} against {read:?}"
        );
        assert!(merge.highlights < 0.0 && merge.whites < 0.0, "{merge:?}");
    }
}
