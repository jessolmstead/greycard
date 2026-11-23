//! The output sharpen ([`greycard_edit::OutputSharpen`]): an unsharp
//! mask on the finished picture's luminance. It runs between the look
//! table and the output transform ([`crate::finish`]), on the picture
//! display-referred in the working space. The luminance is read
//! encoded, through the sRGB curve the point curves work in, so a
//! step's halo is the same size in lightness on its dark and its light
//! side; sharpened in linear light instead, the dark side takes the
//! whole change in the few levels it has and goes to black in a hard
//! line (`linear_light_halos_are_lopsided` measures it). The sharpened
//! luminance goes back on as a ratio on the linear color, so the
//! channels keep their proportions: brightness only, the hue and the
//! saturation as they were.
//!
//! The blur is a Gaussian of the radius in pixels of the picture being
//! made, its edges clamped: an export at 2048 pixels is sharpened at the
//! radius in its own pixels, as the full-size one is in its own, so a
//! web-size export is as crisp as the full-size one rather than the
//! full-size sharpen seen smaller. The viewport takes it in the window's
//! pixels under 1:1, as an export of that size would, and in the
//! full-size picture's past it (`viewport.wgsl`).
//! The threshold is a soft knee on the difference from it, `d³ / (d² +
//! t²)`: half the difference at the threshold, less below it as its
//! square, nearly all of it above, with no step anywhere, so no level
//! is cut off into a contour. A pixel that is not a number is left as
//! it is and gives its neighbors' blurs nothing.

use greycard_edit::OutputSharpen;
use greycard_edit::output_sharpen::AMOUNT_MAX;
use rayon::prelude::*;

use crate::finish::{decode, encode};

/// Luminance weights of the working space, Rec.2020.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// The most taps a side the blur takes: three of the widest radius.
pub const MOST_TAPS: usize = 9;

/// The linear luminance under which a pixel is black and is left so:
/// no ratio lifts it. `viewport.wgsl`'s `OSHARP_BLACK` is this.
pub const BLACK: f32 = 1e-6;

/// The amount over which a pixel's channels under zero are held at
/// zero whole, under AgX's rail ([`Params::hold`]); under it they are
/// held by the share the amount is of it, so the hold comes on with the
/// sharpen and not at its first ulp. A mask's weight sets the amount a
/// pixel, and two devices may round a weight of next to nothing to
/// nothing or not: a hold switched on whole at any amount turned that
/// into whole levels of an out-of-gamut color (the parity sweep's seed
/// 0x67726579 edit 195 on a real card, 3.7 levels of blue). The panel's
/// first step, 1%, so every amount the panel sets is held whole.
/// `viewport.wgsl`'s `OSHARP_HOLD_RAMP` is this.
pub const HOLD_RAMP: f32 = 0.01;

/// The sharpen as the finish of one picture takes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub amount: f32,
    /// The blur's standard deviation in this picture's pixels.
    pub sigma: f32,
    pub threshold: f32,
    /// Whether channels under zero are held at zero ([`held`]): under
    /// AgX's rail (`finish::Baked::agx_railed`), which takes only what
    /// is under zero and clips per channel as it always did, so an AgX
    /// picture is what it was. Where the gamut op follows, a color under
    /// zero is left to it, scaled as any other: a hold there would clip
    /// the sharpened pixels per channel and the rest not, a hue step at
    /// a mask's edge.
    pub hold: bool,
}

impl Params {
    /// These, held at zero or not ([`Params::hold`]).
    pub fn holding(self, hold: bool) -> Self {
        Params { hold, ..self }
    }

    /// `s` for the picture being made, its radius in that picture's own
    /// pixels whatever its size, or none when it does nothing.
    pub fn of(s: &OutputSharpen) -> Option<Self> {
        s.acting().map(|s| Params {
            amount: s.amount,
            sigma: s.radius,
            threshold: s.threshold,
            hold: false,
        })
    }

    /// Taps a side of the blur: what a band of rows needs beyond it.
    pub fn reach(&self) -> usize {
        half_width(self.sigma)
    }
}

/// The settings of their own, beyond the picture's, that the masks'
/// positive shares of one picture sharpen at: each a blur of its own
/// radius at most, so a finish blurs at most four times. A mask's
/// fourth distinct setting and on, in the masks' order, sharpens at the
/// picture's. The viewport holds as many (`osharp.rs`).
pub const MOST_OWN: usize = 3;

/// The masks whose own setting the cap ([`MOST_OWN`]) last sent to the
/// picture's, by their place, with the setting (radius and threshold as
/// bits): a plan is made for every draw and every finish, and the log
/// says it once each time it changes.
struct Dropped(std::sync::Mutex<Vec<(usize, u32, u32)>>);

static DROPPED: Dropped = Dropped(std::sync::Mutex::new(Vec::new()));

impl Dropped {
    /// Says `dropped` in the log if it is not what was said last;
    /// whether it said it.
    fn say(&self, dropped: Vec<(usize, u32, u32)>) -> bool {
        let mut last = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *last == dropped {
            return false;
        }
        for &(i, r, t) in &dropped {
            tracing::info!(
                "output sharpen: mask {} at radius {} and threshold {} is past the {MOST_OWN} \
                 settings of their own a picture takes, and sharpens at the picture's",
                i + 1,
                f32::from_bits(r),
                f32::from_bits(t)
            );
        }
        let said = !dropped.is_empty();
        *last = dropped;
        said
    }
}

/// The output sharpen a finish under its masks runs (§663, §730):
/// unsharp terms summed on the encoded luminance, each its own amount,
/// blur, and threshold. At a pixel, with `w` a mask's weight times its
/// opacity and `s` its share,
///
/// ```text
/// A₀ = clamp(base · Π(1 + w·s⁻) + Σ₀ w·s⁺, 0, 2)
/// Aₖ = clamp(Σₖ w·s⁺, 0, 2)                       k = 1..=3
/// S  = A₀ + Σₖ Aₖ;  where S > 2, every Aᵢ ← Aᵢ · 2 / S
/// δ  = A₀ · soft(l − B₀, t₀) + Σₖ Aₖ · soft(l − Bₖ, tₖ)
/// ```
///
/// Term 0 is the picture's, at its Radius and Threshold: its own
/// Amount, every mask's take (a negative share, which scales the
/// picture's alone), and the positive shares of the masks with no
/// Radius or Threshold of their own, or with the picture's. Term `k` is
/// an own setting's ([`MOST_OWN`]): the positive shares of the masks at
/// that radius and threshold. `Bₖ` is the luminance's blur at the
/// term's radius, and `δ` moves it as one term does ([`apply`]), held at
/// zero under AgX's rail by the amounts' sum. The sum is held at 200%
/// as one term's amount is, so a radius moved off the picture's, or a
/// setting past the cap sharpening at the picture's, changes the
/// radius and not how much a pixel takes. With no own setting this is
/// §730's single term, to the bit.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Term 0: the picture's own amount (none while its section is
    /// off), its radius and threshold held to the ends whether it is on
    /// or off, and the hold.
    pub picture: Params,
    /// Whether term 0 can act anywhere: the picture's amount, or a
    /// mask's positive share at its settings. Not blurred otherwise.
    pub picture_acts: bool,
    /// The own settings, `(radius, threshold)`: the first [`MOST_OWN`]
    /// distinct ones in the masks' order, then sorted by radius and
    /// threshold, so the terms sum in one order whatever the masks'.
    pub own: Vec<(f32, f32)>,
    /// Whether each pixel's amounts are its own ([`Plan::amounts_at`]):
    /// some mask has a share. Otherwise every pixel takes term 0 at the
    /// picture's amount.
    pub shared: bool,
}

impl Plan {
    /// What a finish of the picture's sharpen `s` under `locals` runs,
    /// or none when nothing sharpens: the picture's off and no mask's
    /// share positive (a take alone takes from nothing).
    pub fn finishing(s: &OutputSharpen, locals: &[crate::finish::Local]) -> Option<Self> {
        let held = s.held();
        let picture = Params {
            amount: Params::of(s).map_or(0.0, |p| p.amount),
            sigma: held.radius,
            threshold: held.threshold,
            hold: false,
        };
        let mut plan = Plan {
            picture,
            picture_acts: picture.amount > 0.0,
            own: Vec::new(),
            shared: false,
        };
        // The masks whose own setting the cap sent to the picture's, by
        // their place, with the setting.
        let mut dropped: Vec<(usize, u32, u32)> = Vec::new();
        for (i, l) in locals
            .iter()
            .enumerate()
            .filter(|(_, l)| l.enabled && l.opacity > 0.0)
        {
            let share = l.baked.output_share;
            plan.shared |= share != 0.0;
            if share <= 0.0 {
                continue;
            }
            let key = plan.key(&l.baked);
            if key == (picture.sigma, picture.threshold) || plan.own.contains(&key) {
                plan.picture_acts |= key == (picture.sigma, picture.threshold);
            } else if plan.own.len() < MOST_OWN {
                plan.own.push(key);
            } else {
                plan.picture_acts = true;
                dropped.push((i, key.0.to_bits(), key.1.to_bits()));
            }
        }
        DROPPED.say(dropped);
        // By radius, then threshold: the terms sum in one order whatever
        // the masks' order, so two masks swapped are the same picture to
        // the bit.
        plan.own
            .sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        (plan.picture_acts || !plan.own.is_empty()).then_some(plan)
    }

    /// These, held at zero under AgX's rail or not ([`Params::hold`]).
    pub fn holding(self, hold: bool) -> Self {
        Plan {
            picture: self.picture.holding(hold),
            ..self
        }
    }

    /// The radius and threshold a local's positive share sharpens at.
    fn key(&self, b: &crate::finish::Baked) -> (f32, f32) {
        (
            b.output_radius.unwrap_or(self.picture.sigma),
            b.output_threshold.unwrap_or(self.picture.threshold),
        )
    }

    /// The term a local's share goes to: 0, the picture's, for a take,
    /// for a share at the picture's settings, and past [`MOST_OWN`]; `k`
    /// for the own setting `k - 1`.
    pub fn term_of(&self, b: &crate::finish::Baked) -> usize {
        if b.output_share <= 0.0 {
            return 0;
        }
        let key = self.key(b);
        self.own.iter().position(|k| *k == key).map_or(0, |k| k + 1)
    }

    /// Amounts a pixel: term 0 and each own setting's.
    pub fn terms(&self) -> usize {
        1 + self.own.len()
    }

    /// The terms' amounts at a pixel into `out` ([`Self::terms`] long),
    /// from each local's share under its weight `w` there. A share under
    /// zero scales the picture's amount by `1 + w·share`, so -1 is none
    /// of the picture's under the mask whatever its amount, and -0.5
    /// half; one over zero adds `w·share` to its term, so +1 is 100%
    /// more, and over the picture's off 100% under it alone. With
    /// several masks the scales multiply and the additions sum, the
    /// scales on the picture's amount before the additions: each mask's
    /// take is of the picture's sharpening, never of another mask's,
    /// and the order of the masks does not count. Each held to the
    /// amount's ends, and their sum held at the top by scaling every
    /// term alike.
    #[inline]
    pub fn amounts_at(&self, locals: &[(&crate::finish::Baked, f32)], out: &mut [f32]) {
        let mut scale = 1.0;
        out.fill(0.0);
        for (b, w) in locals {
            let s = w * b.output_share;
            if s < 0.0 {
                scale *= (1.0 + s).max(0.0);
            } else if self.own.is_empty() {
                out[0] += s;
            } else {
                out[self.term_of(b)] += s;
            }
        }
        out[0] = (self.picture.amount * scale + out[0]).clamp(0.0, AMOUNT_MAX);
        if self.own.is_empty() {
            return;
        }
        // The sum held at the amount's top, every term scaled alike: so
        // a mask's radius moved off the picture's, or past the cap onto
        // it, changes the radius and not how much the pixel takes. With
        // no own term the clamp above is this.
        let mut sum = out[0];
        for a in &mut out[1..] {
            *a = a.clamp(0.0, AMOUNT_MAX);
            sum += *a;
        }
        if sum > AMOUNT_MAX {
            let held = AMOUNT_MAX / sum;
            for a in out.iter_mut() {
                *a *= held;
            }
        }
    }

    /// Taps a side of the widest blur: what a band of rows needs beyond
    /// it.
    pub fn reach(&self) -> usize {
        let own = self.own.iter().map(|&(r, _)| half_width(r));
        own.chain(self.picture_acts.then(|| self.picture.reach()))
            .max()
            .unwrap_or(1)
    }

    /// The blurs the terms read, by radius, each once: term 0's first
    /// where it acts, then each own radius not yet among them; and
    /// which of them each term reads (none for term 0 where it does
    /// not act).
    pub fn blurs(&self) -> (Vec<f32>, Vec<Option<usize>>) {
        let mut radii: Vec<f32> = Vec::new();
        let of = |r: f32, radii: &mut Vec<f32>| match radii.iter().position(|x| *x == r) {
            Some(i) => i,
            None => {
                radii.push(r);
                radii.len() - 1
            }
        };
        let mut reads = vec![
            self.picture_acts
                .then(|| of(self.picture.sigma, &mut radii)),
        ];
        for &(r, _) in &self.own {
            reads.push(Some(of(r, &mut radii)));
        }
        (radii, reads)
    }

    /// The threshold each term sharpens at.
    pub fn thresholds(&self) -> impl Iterator<Item = f32> + '_ {
        std::iter::once(self.picture.threshold).chain(self.own.iter().map(|&(_, t)| t))
    }
}

/// §730's amount at a pixel, one term at the picture's radius, as it
/// was written before a mask had settings of its own: the reference
/// [`Plan::amounts_at`]'s term 0 is held to, to the bit.
#[cfg(test)]
pub(crate) fn amount_at(base: f32, locals: &[(&crate::finish::Baked, f32)]) -> f32 {
    let (mut scale, mut add) = (1.0, 0.0);
    for (b, w) in locals {
        let s = w * b.output_share;
        if s < 0.0 {
            scale *= (1.0 + s).max(0.0);
        } else {
            add += s;
        }
    }
    (base * scale + add).clamp(0.0, AMOUNT_MAX)
}

/// Taps a side for a blur of `sigma`: three of it, one at least.
pub fn half_width(sigma: f32) -> usize {
    ((3.0 * sigma).ceil() as usize).clamp(1, MOST_TAPS)
}

/// The blur's weights from the middle out, the middle first, as the
/// shader makes them: not summed to one, since a blur divides by the
/// weights of the taps it read.
pub fn kernel(sigma: f32) -> Vec<f32> {
    let s = sigma.max(1e-3);
    (0..=half_width(sigma))
        .map(|i| (-((i * i) as f32) / (2.0 * s * s)).exp())
        .collect()
}

/// A color's luminance, encoded: what the sharpen reads. Not a number
/// when the color is not finite.
#[inline]
pub fn luminance(c: [f32; 3]) -> f32 {
    if !c.iter().all(|v| v.is_finite()) {
        return f32::NAN;
    }
    encode((LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2]).clamp(0.0, 1.0))
}

/// The share of a difference `d` that is sharpened under threshold
/// `t`: the soft knee.
#[inline]
pub fn soft(d: f32, t: f32) -> f32 {
    let d2 = d * d;
    if t <= 0.0 || d2 == 0.0 {
        d
    } else {
        d * d2 / (d2 + t * t)
    }
}

/// `c` with luminance `l` sharpened against its blur `blurred`: the
/// encoded luminance moved, and the linear color scaled by what that
/// does to the linear luminance. Black stays black; a channel under
/// zero is scaled as any other, its proportions kept, for the gamut op
/// to bring in at its hue, or, held ([`Params::hold`]), no channel goes
/// under zero on any pixel the sharpen acts on at [`HOLD_RAMP`] or
/// more, moved or not (under it, by the amount's share); and a
/// brightening stops where the color's largest channel reaches white,
/// so the channels' proportions hold. At no amount (a pixel outside
/// the only mask that sharpens, or under one that takes the picture's
/// all off) the pixel is not sharpened and is as it came, to the bit.
#[inline]
pub fn apply(c: [f32; 3], l: f32, blurred: f32, p: &Params) -> [f32; 3] {
    moved(
        c,
        l,
        p.amount,
        p.amount * soft(l - blurred, p.threshold),
        p.hold,
    )
}

/// `c` with luminance `l` moved by `delta` in encoded luminance, as
/// [`apply`] moves it: `amount` the terms' amounts summed, which says
/// whether the pixel is sharpened at all and how far it is held.
#[inline]
pub fn moved(c: [f32; 3], l: f32, amount: f32, delta: f32, hold: bool) -> [f32; 3] {
    if !c.iter().all(|v| v.is_finite()) || amount.is_nan() || amount <= 0.0 {
        return c;
    }
    // Held, every pixel the sharpen passes over is held at zero from
    // below, moved or not, so a flat stretch outside the gamut is
    // clipped as its sharpened edge is: whole from `HOLD_RAMP` up, by
    // the amount's share of it under that.
    let c = if hold { held(c, amount) } else { c };
    if delta == 0.0 || !delta.is_finite() {
        return c;
    }
    let base = decode(l);
    if base <= BLACK {
        return c;
    }
    let mut ratio = decode((l + delta).clamp(0.0, 1.0)) / base;
    let top = c[0].max(c[1]).max(c[2]);
    if ratio > 1.0 && top > 0.0 {
        ratio = ratio.min((1.0 / top).max(1.0));
    }
    c.map(|v| v * ratio)
}

/// `c` held at zero from below at `amount`: whole from [`HOLD_RAMP`]
/// up, and under it each channel under zero moved toward zero by the
/// amount's share of the ramp.
#[inline]
pub fn held(c: [f32; 3], amount: f32) -> [f32; 3] {
    if amount >= HOLD_RAMP {
        return c.map(|v| v.max(0.0));
    }
    let f = (amount / HOLD_RAMP).clamp(0.0, 1.0);
    c.map(|v| if v < 0.0 { v - f * v } else { v })
}

/// One tap a side of `x` in a line `len` long, clamped at its ends,
/// read through `at`; the taps that are not numbers left out of the
/// sum and their weight with them.
#[inline]
fn blur_at(k: &[f32], x: usize, len: usize, at: impl Fn(usize) -> f32) -> f32 {
    let n = k.len() - 1;
    let (mut acc, mut sum) = (0.0f32, 0.0f32);
    for i in 0..=2 * n {
        let j = (x + i).saturating_sub(n).min(len - 1);
        let v = at(j);
        if v.is_finite() {
            let w = k[i.abs_diff(n)];
            acc += w * v;
            sum += w;
        }
    }
    if sum > 0.0 { acc / sum } else { f32::NAN }
}

/// `plane`, `w` wide and `h` tall, through the Gaussian of `sigma`,
/// its edges clamped: across, then down.
pub fn blur(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    blur_rows(plane, w, h, 0, h, sigma)
}

/// [`blur`] of `plane`'s `rows` tall picture at rows `first` to
/// `first + count` alone: the rows outside them read for the blur down
/// and its edges clamped at `plane`'s own.
fn blur_rows(
    plane: &[f32],
    w: usize,
    rows: usize,
    first: usize,
    count: usize,
    sigma: f32,
) -> Vec<f32> {
    let k = kernel(sigma);
    let n = k.len() - 1;
    // Across only the rows the blur down reads.
    let lo = first.saturating_sub(n);
    let hi = (first + count + n).min(rows);
    let mut across = vec![0.0f32; w * (hi - lo)];
    across
        .par_chunks_mut(w)
        .zip(plane[lo * w..hi * w].par_chunks(w))
        .for_each(|(out, row)| {
            for (x, o) in out.iter_mut().enumerate() {
                *o = blur_at(&k, x, w, |j| row[j]);
            }
        });
    let mut out = vec![0.0f32; w * count];
    out.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
        let y = first + r;
        for (x, o) in row.iter_mut().enumerate() {
            *o = blur_at(&k, y, rows, |j| across[(j - lo) * w + x]);
        }
    });
    out
}

/// The sharpen over a picture's colors, `w` by `h`, in place.
pub fn sharpen(colors: &mut [[f32; 3]], w: usize, h: usize, p: &Params) {
    let sharpened = sharpen_rows(colors, w, h, 0, h, p, None);
    colors.copy_from_slice(&sharpened);
}

/// The sharpened colors of rows `first` to `first + count` of `held`,
/// a picture `w` wide and `rows` tall: a band of the frame with the
/// rows its blur reads above and below it, or the whole of it. The
/// picture's edges are `held`'s, so a band held with [`Params::reach`]
/// rows each side (fewer at the frame's own edges) comes out as the
/// same rows of the whole frame do, to the bit. `amounts`, when given,
/// is each of those rows' pixels' own amount, `p`'s otherwise.
pub(crate) fn sharpen_rows(
    held: &[[f32; 3]],
    w: usize,
    rows: usize,
    first: usize,
    count: usize,
    p: &Params,
    amounts: Option<&[f32]>,
) -> Vec<[f32; 3]> {
    let l: Vec<f32> = held.par_iter().map(|c| luminance(*c)).collect();
    let b = blur_rows(&l, w, rows, first, count, p.sigma);
    let band = first * w..(first + count) * w;
    held[band.clone()]
        .par_iter()
        .zip(l[band].par_iter().zip(b.par_iter()))
        .enumerate()
        .map(|(i, (c, (l, b)))| match amounts {
            Some(a) => apply(*c, *l, *b, &Params { amount: a[i], ..*p }),
            None => apply(*c, *l, *b, p),
        })
        .collect()
}

/// [`sharpen_rows`] by `plan`'s terms ([`Plan`]): `amounts`, when
/// given, each pixel's [`Plan::terms`] amounts in a row, term 0 at the
/// picture's amount everywhere otherwise. With no own setting this is
/// [`sharpen_rows`] of term 0, to the bit; with one, a pixel where only
/// term 0 has an amount is what that makes it, to the bit.
pub(crate) fn sharpen_plan_rows(
    held: &[[f32; 3]],
    w: usize,
    rows: usize,
    first: usize,
    count: usize,
    plan: &Plan,
    amounts: Option<&[f32]>,
) -> Vec<[f32; 3]> {
    if plan.own.is_empty() {
        return sharpen_rows(held, w, rows, first, count, &plan.picture, amounts);
    }
    let amounts = amounts.expect("a mask's own setting makes each pixel's amounts its own");
    let l: Vec<f32> = held.par_iter().map(|c| luminance(*c)).collect();
    let (radii, reads) = plan.blurs();
    let blurs: Vec<Vec<f32>> = radii
        .iter()
        .map(|&r| blur_rows(&l, w, rows, first, count, r))
        .collect();
    let thresholds: Vec<f32> = plan.thresholds().collect();
    let n = plan.terms();
    let band = first * w..(first + count) * w;
    held[band.clone()]
        .par_iter()
        .zip(l[band].par_iter())
        .enumerate()
        .map(|(i, (c, l))| {
            let a = &amounts[i * n..(i + 1) * n];
            let (mut amount, mut delta) = (0.0f32, 0.0f32);
            for (k, (&a, read)) in a.iter().zip(&reads).enumerate() {
                if let (true, Some(b)) = (a > 0.0, read) {
                    amount += a;
                    delta += a * soft(l - blurs[*b][i], thresholds[k]);
                }
            }
            moved(*c, *l, amount, delta, plan.picture.hold)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(amount: f32, radius: f32, threshold: f32) -> OutputSharpen {
        OutputSharpen {
            enabled: true,
            amount,
            radius,
            threshold,
        }
    }

    /// A grey step across the middle of a `w` wide, `h` tall picture,
    /// `dark` to `light`, encoded values, as the finish holds colors.
    fn step(w: usize, h: usize, dark: f32, light: f32) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| {
                let v = decode(if i % w < w / 2 { dark } else { light });
                [v, v, v]
            })
            .collect()
    }

    #[test]
    fn nothing_at_no_amount_and_the_radius_is_the_pictures_own() {
        assert_eq!(Params::of(&OutputSharpen::default()), None);
        assert_eq!(Params::of(&on(0.5, 1.0, 0.0).with_off()), None);
        let p = Params::of(&on(0.5, 2.0, 0.01)).unwrap();
        assert_eq!(p.sigma, 2.0);
        let k = kernel(1.0);
        assert_eq!((k.len(), k[0]), (4, 1.0));
        // A flat line stays the value it is, the weights summed out.
        let flat = blur(&[0.37; 12 * 5], 12, 5, 1.3);
        assert!(flat.iter().all(|v| (v - 0.37).abs() < 1e-6));
        assert_eq!(half_width(3.0), MOST_TAPS);
    }

    trait Off {
        fn with_off(self) -> Self;
    }
    impl Off for OutputSharpen {
        fn with_off(self) -> Self {
            OutputSharpen {
                enabled: false,
                ..self
            }
        }
    }

    /// A step is steepened, its two sides pushed apart by the same
    /// lightness; a flat stays flat, and a grey stays grey.
    #[test]
    fn a_step_steepens_and_a_flat_keeps() {
        let (w, h) = (32, 4);
        let mut px = step(w, h, 0.3, 0.7);
        let p = Params::of(&on(1.0, 1.0, 0.0)).unwrap();
        sharpen(&mut px, w, h, &p);
        let e = |x: usize| encode(px[w + x][0]);
        assert!(e(w / 2 - 1) < 0.3 - 0.05, "{}", e(w / 2 - 1));
        assert!(e(w / 2) > 0.7 + 0.05, "{}", e(w / 2));
        // The halo is as deep on the dark side as it is high on the
        // light one.
        let (under, over) = (0.3 - e(w / 2 - 1), e(w / 2) - 0.7);
        assert!((under - over).abs() < 1e-3, "{under} {over}");
        // Far from the step, nothing.
        assert!((e(2) - 0.3).abs() < 1e-5 && (e(w - 3) - 0.7).abs() < 1e-5);
        assert!(px.iter().all(|c| c[0] == c[1] && c[1] == c[2]));
    }

    /// Under the threshold a difference is kept back, softly: half at
    /// the threshold, a ninth at a third of it, and the change never
    /// jumps as the difference grows.
    #[test]
    fn the_threshold_is_a_soft_knee() {
        let t = 0.02;
        assert!((soft(t, t) - t / 2.0).abs() < 1e-7);
        assert!((soft(t / 3.0, t) - (t / 3.0) / 10.0).abs() < 1e-7);
        assert!((soft(-t, t) + t / 2.0).abs() < 1e-7);
        assert!(soft(10.0 * t, t) > 0.99 * 10.0 * t);
        let mut last = 0.0;
        for i in 1..2000 {
            let d = i as f32 * 1e-4;
            let s = soft(d, t);
            assert!(s > last && s - last < 2e-4, "at {d}");
            last = s;
        }
        // A low-contrast step under it moves far less than with none.
        let (w, h) = (32, 2);
        let moved = |threshold: f32| {
            let mut px = step(w, h, 0.50, 0.51);
            let p = Params::of(&on(1.0, 1.0, threshold)).unwrap();
            sharpen(&mut px, w, h, &p);
            encode(px[w / 2][0]) - 0.51
        };
        assert!(
            moved(0.05) < 0.1 * moved(0.0),
            "{} {}",
            moved(0.05),
            moved(0.0)
        );
    }

    /// Why the luminance is encoded: sharpened in linear light at the
    /// amount that lifts a step's light side as much, the dark side
    /// goes down further in lightness, the more so the darker it is.
    /// Measured here, encoded halos against linear ones on steps from
    /// shadow to highlight, none of them clipped.
    #[test]
    fn linear_light_halos_are_lopsided() {
        let (w, h) = (32, 2);
        for (dark, light) in [(0.25f32, 0.6f32), (0.35, 0.75), (0.45, 0.85), (0.15, 0.4)] {
            let mut px = step(w, h, dark, light);
            sharpen(&mut px, w, h, &Params::of(&on(0.5, 1.0, 0.0)).unwrap());
            let enc = (
                dark - encode(px[w / 2 - 1][0]),
                encode(px[w / 2][0]) - light,
            );
            // Linear light: the same unsharp mask on linear luminance,
            // its amount set so the light side lifts as far.
            let lin: Vec<f32> = step(w, h, dark, light).iter().map(|c| c[0]).collect();
            let b = blur(&lin, w, h, 1.0);
            let d = |x: usize| lin[x] - b[x];
            let over = |a: f32| encode((lin[w / 2] + a * d(w / 2)).clamp(0.0, 1.0)) - light;
            let (mut lo, mut hi) = (0.0f32, 20.0f32);
            for _ in 0..60 {
                let mid = 0.5 * (lo + hi);
                if over(mid) < enc.1 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let a = 0.5 * (lo + hi);
            let under = dark - encode((lin[w / 2 - 1] + a * d(w / 2 - 1)).clamp(0.0, 1.0));
            eprintln!(
                "step {dark}-{light}: encoded halos {:.3} under, {:.3} over; \
                 linear {under:.3} under at amount {a:.2}",
                enc.0, enc.1
            );
            assert!(under > 1.5 * enc.0, "{under} against {}", enc.0);
        }
    }

    /// The sharpened luminance goes on as a ratio: on saturated edges
    /// the channels keep their proportions, so the hue and the
    /// saturation are what they were, dark halo and light alike, and
    /// no channel goes under zero or a color past white.
    #[test]
    fn hue_and_saturation_hold_on_a_saturated_edge() {
        let (w, h) = (24, 3);
        for (dark, light) in [
            ([0.02f32, 0.004, 0.001], [0.9, 0.35, 0.05]),
            ([0.05, 0.01, 0.2], [0.2, 0.6, 0.9]),
            ([0.0, 0.03, 0.0], [0.95, 0.4, 0.0]),
        ] {
            let mut px: Vec<[f32; 3]> = (0..w * h)
                .map(|i| if i % w < w / 2 { dark } else { light })
                .collect();
            let before = px.clone();
            sharpen(&mut px, w, h, &Params::of(&on(2.0, 1.5, 0.0)).unwrap());
            let mut moved = 0;
            for (a, b) in before.iter().zip(&px) {
                assert!(b.iter().all(|v| (0.0..=1.0).contains(v)), "{b:?}");
                if a != b {
                    moved += 1;
                }
                let (ta, tb) = (a[0] + a[1] + a[2], b[0] + b[1] + b[2]);
                // A dark side taken all the way to black has no hue left
                // to keep.
                if tb == 0.0 {
                    continue;
                }
                for k in 0..3 {
                    assert!((a[k] / ta - b[k] / tb).abs() < 1e-5, "{a:?} went to {b:?}");
                }
            }
            assert!(moved >= 2 * h, "the edge moved");
        }
        // Black stays black, however bright its neighbors.
        let mut px = vec![[0.0f32; 3]; 8];
        px[4..].fill([0.8, 0.8, 0.8]);
        sharpen(&mut px, 8, 1, &Params::of(&on(2.0, 1.0, 0.0)).unwrap());
        assert_eq!(px[3], [0.0; 3]);
    }

    /// A pixel that is not a number stays as it is and gives the blur
    /// nothing: its neighbors come out as they would beside a pixel
    /// that is not there, not spread with it.
    #[test]
    fn a_pixel_that_is_not_a_number_stays_itself_and_spreads_nothing() {
        let (w, h) = (16, 16);
        let p = Params::of(&on(1.0, 2.0, 0.0)).unwrap();
        let mut px = step(w, h, 0.3, 0.6);
        px[8 * w + 3] = [f32::NAN, 0.2, 0.2];
        px[2 * w + 12] = [f32::INFINITY; 3];
        sharpen(&mut px, w, h, &p);
        assert!(px[8 * w + 3][0].is_nan() && px[8 * w + 3][1] == 0.2);
        assert_eq!(px[2 * w + 12], [f32::INFINITY; 3]);
        assert!(
            px.iter()
                .enumerate()
                .filter(|(i, _)| *i != 8 * w + 3 && *i != 2 * w + 12)
                .all(|(_, c)| c.iter().all(|v| v.is_finite()))
        );
        // A flat field with a hole sharpens to the same flat field.
        let mut flat = vec![[0.4f32; 3]; w * h];
        flat[5 * w + 5] = [f32::NAN; 3];
        sharpen(&mut flat, w, h, &p);
        assert!(
            flat.iter()
                .enumerate()
                .filter(|(i, _)| *i != 5 * w + 5)
                .all(|(_, c)| (c[0] - 0.4).abs() < 1e-6)
        );
    }

    /// Held (AgX's rail), a channel under zero (a color outside the
    /// working space) is held at zero on every pixel the sharpen passes over, the flat ones
    /// too, so a flat stretch is clipped as its sharpened edge is.
    #[test]
    fn every_pixel_is_held_at_zero_from_below_alike() {
        let (w, h) = (24, 2);
        let wide = [0.5f32, -0.02, 0.3];
        let mut px: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                if i % w < w / 2 {
                    wide
                } else {
                    [0.05, 0.05, 0.05]
                }
            })
            .collect();
        let p = Params::of(&on(1.0, 1.0, 0.01)).unwrap().holding(true);
        sharpen(&mut px, w, h, &p);
        assert_eq!(px[0], [0.5, 0.0, 0.3], "a flat pixel far from the edge");
        assert!(px.iter().all(|c| c.iter().all(|v| *v >= 0.0)));
        assert_ne!(px[w / 2 - 1], [0.5, 0.0, 0.3], "the edge moved");
    }

    /// Held (AgX's rail), the hold at zero comes on with the amount: a color outside the
    /// gamut at an amount of next to nothing is next to as it came, not
    /// clipped (the parity sweep's edit 195 on a real card, where a
    /// mask's weight of next to nothing on one device and nothing on
    /// another made 3.7 levels of blue); from `HOLD_RAMP` up it is held
    /// whole, as before; and the sharpen is continuous in its amount
    /// all the way up from none.
    #[test]
    fn the_hold_at_zero_comes_on_with_the_amount() {
        let c = [0.2, -0.03, 0.05];
        let (l, blurred) = (luminance([0.2, 0.0, 0.05]), 0.3);
        let at = |amount: f32| {
            apply(
                c,
                l,
                blurred,
                &Params {
                    amount,
                    sigma: 1.0,
                    threshold: 0.01,
                    hold: true,
                },
            )
        };
        let gap =
            |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max);
        assert_eq!(at(0.0), c);
        assert!(gap(at(1e-6), c) < 1e-5, "{:?}", at(1e-6));
        assert!(gap(at(1e-4), c) < 1e-3, "{:?}", at(1e-4));
        assert_eq!(held(c, HOLD_RAMP), [0.2, 0.0, 0.05]);
        assert_eq!(held(c, 1.0), [0.2, 0.0, 0.05]);
        assert_eq!(held(c, HOLD_RAMP / 2.0)[1], -0.015);
        assert!(at(1.0)[1] >= 0.0, "held whole at a full amount");
        // No step anywhere along the amount.
        let mut last = at(0.0);
        for i in 1..=2000 {
            let now = at(i as f32 * 0.0001);
            assert!(gap(now, last) < 1e-3, "a step at {}", i as f32 * 0.0001);
            last = now;
        }
    }

    /// A channel under zero is not held: the color is scaled as any
    /// other, its proportions kept, for the gamut op to bring in at its
    /// hue, and the sharpen comes on with its amount from the color as
    /// it came, with no step.
    #[test]
    fn a_color_under_zero_is_left_to_the_gamut_op() {
        let c = [0.2, -0.03, 0.05];
        let (l, blurred) = (luminance(c), 0.3);
        let past = Params {
            amount: 1.0,
            sigma: 1.0,
            threshold: 0.01,
            hold: false,
        };
        let out = apply(c, l, blurred, &past);
        let ratio = out[0] / c[0];
        assert!(ratio < 1.0, "darkened against a brighter blur: {ratio}");
        for k in 0..3 {
            assert!((out[k] - c[k] * ratio).abs() < 1e-7, "{out:?}");
        }
        // And it comes on with the amount, from the color as it came.
        let at = |amount: f32| apply(c, l, blurred, &Params { amount, ..past });
        assert_eq!(at(0.0), c);
        let mut last = c;
        for i in 1..=1000 {
            let now = at(i as f32 * 0.001);
            assert!(
                (0..3).all(|k| (now[k] - last[k]).abs() < 1e-3),
                "a step at {i}"
            );
            last = now;
        }
    }

    /// An amount a pixel: where it is the picture's the pixel is what
    /// the picture's sharpen makes it, to the bit; where it is none the
    /// pixel is as it came, to the bit, a channel under zero kept; and
    /// the amounts are taken by scale and added by weight, held to the
    /// ends (`amount_at`).
    #[test]
    fn an_amount_a_pixel_is_the_pictures_or_none() {
        let (w, h) = (24, 6);
        let p = Plan::finishing(&on(1.2, 1.4, 0.01), &[]).unwrap().picture;
        let mut px: Vec<[f32; 3]> = step(w, h, 0.3, 0.7);
        px[3 * w + 2] = [0.4, -0.01, 0.2];
        let whole = sharpen_rows(&px, w, h, 0, h, &p, None);
        let amounts: Vec<f32> = (0..w * h)
            .map(|i| if i % w < w / 2 + 1 { 0.0 } else { 1.2 })
            .collect();
        let by = sharpen_rows(&px, w, h, 0, h, &p, Some(&amounts));
        for i in 0..w * h {
            let want = if amounts[i] == 0.0 { px[i] } else { whole[i] };
            assert_eq!(by[i], want, "pixel {i}");
        }
        assert_ne!(whole[w / 2 + 1], px[w / 2 + 1], "the edge moved");
        // Off, the blur is still there for a mask's share, at the
        // picture's radius and threshold and no amount of its own.
        let off = on(1.2, 1.4, 0.01).with_off();
        let masked = |share: f32| {
            let mut a = greycard_edit::Adjustment::default();
            a.mask.components.push(Default::default());
            a.look.detail.output_sharpen = share;
            crate::finish::Local::of(&a, vec![None], None)
        };
        assert_eq!(Plan::finishing(&off, &[]), None);
        assert_eq!(Plan::finishing(&off, &[masked(-0.5)]), None);
        assert_eq!(
            Plan::finishing(&off, &[masked(-0.5), masked(0.3)]),
            Some(Plan {
                picture: Params {
                    amount: 0.0,
                    sigma: 1.4,
                    threshold: 0.01,
                    hold: false,
                },
                picture_acts: true,
                own: Vec::new(),
                shared: true,
            })
        );
        let picture = on(1.2, 1.4, 0.01);
        assert_eq!(
            Plan::finishing(&picture, &[]).map(|p| p.shared),
            Some(false)
        );
        assert_eq!(
            Plan::finishing(&picture, &[masked(-0.5)]).map(|p| p.shared),
            Some(true)
        );
        let local = |share: f32| crate::finish::Baked {
            output_share: share,
            ..crate::finish::Baked::of(&greycard_edit::Look::default())
        };
        let (a, b, off) = (local(1.0), local(-0.5), local(-1.0));
        // The plan's term 0 is this, to the bit, with no own setting.
        let at_picture = |base: f32| Plan {
            picture: Params { amount: base, ..p },
            picture_acts: true,
            own: Vec::new(),
            shared: true,
        };
        for (base, on) in [
            (0.0, vec![(&a, 0.25)]),
            (1.0, vec![(&a, 1.0), (&a, 1.0)]),
            (2.0, vec![(&off, 1.0)]),
            (1.0, vec![(&b, 0.5)]),
            (1.5, vec![(&off, 1.0), (&a, 0.5)]),
            (1.0, vec![(&b, 1.0), (&a, 0.25)]),
            (0.7, vec![(&b, 0.3), (&a, 0.37), (&b, 0.9), (&a, 0.11)]),
        ] {
            let mut out = [0.0];
            at_picture(base).amounts_at(&on, &mut out);
            assert_eq!(out[0].to_bits(), amount_at(base, &on).to_bits());
        }
        assert_eq!(amount_at(0.0, &[(&a, 0.25)]), 0.25);
        assert_eq!(amount_at(1.0, &[(&a, 1.0), (&a, 1.0)]), AMOUNT_MAX);
        // A take scales the picture's: -100 is none at any amount,
        // -50 half, under a half weight a quarter off.
        assert_eq!(amount_at(2.0, &[(&off, 1.0)]), 0.0);
        assert_eq!(amount_at(2.0, &[(&b, 1.0)]), 1.0);
        assert_eq!(amount_at(1.0, &[(&b, 0.5)]), 0.75);
        // Over the picture's off, a take does nothing.
        assert_eq!(amount_at(0.0, &[(&off, 1.0)]), 0.0);
        // Takes multiply: two halves leave a quarter.
        assert_eq!(amount_at(2.0, &[(&b, 1.0), (&b, 1.0)]), 0.5);
        // A take is of the picture's alone, not of a mask's addition,
        // in either order.
        assert_eq!(amount_at(1.5, &[(&off, 1.0), (&a, 0.5)]), 0.5);
        assert_eq!(amount_at(1.5, &[(&a, 0.5), (&off, 1.0)]), 0.5);
        assert_eq!(amount_at(1.0, &[(&b, 1.0), (&a, 0.25)]), 0.75);
    }

    /// A band of rows held with the blur's reach above and below comes
    /// out as those rows of the whole frame, to the bit.
    #[test]
    fn a_band_is_the_whole_frames_rows() {
        let (w, h) = (20, 41);
        let p = Params::of(&on(1.3, 1.7, 0.01)).unwrap();
        let px: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = ((i * 2654435761) % 1000) as f32 / 1000.0;
                [v, v * 0.7, 0.2]
            })
            .collect();
        let whole = sharpen_rows(&px, w, h, 0, h, &p, None);
        let n = p.reach();
        for (y0, count) in [(0, 6), (6, 6), (17, 9), (35, 6)] {
            let lo = y0 - n.min(y0);
            let hi = (y0 + count + n).min(h);
            let band = sharpen_rows(&px[lo * w..hi * w], w, hi - lo, y0 - lo, count, &p, None);
            assert_eq!(band, whole[y0 * w..(y0 + count) * w], "rows {y0} on");
        }
    }

    /// A local with an Output sharpen share and, maybe, its own Radius
    /// and Threshold, as the finish bakes it.
    fn masked(share: f32, radius: Option<f32>, threshold: Option<f32>) -> crate::finish::Local {
        let mut a = greycard_edit::Adjustment::default();
        a.mask.components.push(Default::default());
        a.look.detail.output_sharpen = share;
        a.look.detail.output_sharpen_radius = radius;
        a.look.detail.output_sharpen_threshold = threshold;
        crate::finish::Local::of(&a, vec![None], None)
    }

    /// Which term each mask's share goes to: a take, a share with no
    /// settings of its own, and one with the picture's own, to the
    /// picture's; one with its own Radius or Threshold to a term of its
    /// own, shared by every mask at the same two; three own settings at
    /// most, a fourth at the picture's. Each radius is blurred once.
    #[test]
    fn a_masks_own_settings_are_a_term_of_their_own() {
        let picture = on(1.0, 1.3, 0.005);
        let locals = [
            masked(0.5, Some(3.0), None),
            masked(0.5, Some(1.3), Some(0.005)),
            masked(-0.5, Some(3.0), Some(0.0)),
            masked(1.0, None, Some(0.0)),
            masked(0.2, Some(3.0), None),
            masked(2.0, Some(0.5), Some(0.1)),
            masked(1.0, Some(2.0), None),
            masked(0.0, Some(2.5), None),
        ];
        let plan = Plan::finishing(&picture, &locals).unwrap();
        assert_eq!(plan.own, vec![(0.5, 0.1), (1.3, 0.0), (3.0, 0.005)]);
        assert!(plan.picture_acts && plan.shared);
        let terms: Vec<usize> = locals.iter().map(|l| plan.term_of(&l.baked)).collect();
        assert_eq!(terms, vec![3, 0, 0, 2, 3, 1, 0, 0]);
        // The picture's radius blurred once for terms 0 and 2.
        assert_eq!(
            plan.blurs(),
            (
                vec![1.3, 0.5, 3.0],
                vec![Some(0), Some(1), Some(0), Some(2)]
            )
        );
        assert_eq!(plan.reach(), MOST_TAPS);
        // Held to the sliders' ends as baked.
        let wide = masked(1.0, Some(9.0), Some(-1.0));
        let plan = Plan::finishing(&picture.with_off(), std::slice::from_ref(&wide)).unwrap();
        assert_eq!(plan.own, vec![(3.0, 0.0)]);
        // The picture's off and only own settings: its radius is not
        // blurred, and the reach is the own one's.
        let only = [masked(1.0, Some(0.5), None)];
        let plan = Plan::finishing(&picture.with_off(), &only).unwrap();
        assert!(!plan.picture_acts);
        assert_eq!(plan.blurs(), (vec![0.5], vec![None, Some(0)]));
        assert_eq!(plan.reach(), 2);
        // A take with its own radius over the picture's off: nothing.
        assert_eq!(
            Plan::finishing(&picture.with_off(), &[masked(-1.0, Some(3.0), None)]),
            None
        );
        // Off under a Detail switched off.
        let mut a = greycard_edit::Adjustment::default();
        a.mask.components.push(Default::default());
        a.look.detail.output_sharpen = 1.0;
        a.look.detail.output_sharpen_radius = Some(3.0);
        a.look.detail.enabled = false;
        let off = crate::finish::Local::of(&a, vec![None], None);
        assert_eq!(Plan::finishing(&picture.with_off(), &[off]), None);
    }

    /// The terms' amounts: the takes scale the picture's term alone, an
    /// own term is its shares summed, each held at 200%, and where the
    /// terms sum past 200% every one is scaled alike to 200%.
    #[test]
    fn own_terms_sum_their_shares_and_the_takes_leave_them() {
        let picture = on(1.0, 1.3, 0.005);
        let locals = [
            masked(-0.5, None, None),
            masked(1.5, Some(3.0), None),
            masked(1.0, Some(3.0), None),
            masked(2.0, None, Some(0.0)),
        ];
        let plan = Plan::finishing(&picture, &locals).unwrap();
        let at = |on: &[(usize, f32)]| {
            let on: Vec<(&crate::finish::Baked, f32)> =
                on.iter().map(|&(i, w)| (&locals[i].baked, w)).collect();
            let mut out = vec![0.0; plan.terms()];
            plan.amounts_at(&on, &mut out);
            out
        };
        assert_eq!(at(&[]), vec![1.0, 0.0, 0.0]);
        assert_eq!(at(&[(0, 1.0), (1, 0.5)]), vec![0.5, 0.0, 0.75]);
        assert_eq!(at(&[(3, 0.25)]), vec![1.0, 0.5, 0.0]);
        // Past the top, held: 1 + 2 (its 2.5 held at 2) is 3, scaled by
        // two thirds; 0.5 + 2 is 2.5, by four fifths.
        let near = |got: Vec<f32>, want: [f32; 3]| {
            assert!(
                got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-6),
                "{got:?} against {want:?}"
            );
            assert!((got.iter().sum::<f32>() - AMOUNT_MAX).abs() < 1e-6);
        };
        near(at(&[(1, 1.0), (2, 1.0)]), [2.0 / 3.0, 0.0, 4.0 / 3.0]);
        near(at(&[(0, 1.0), (3, 1.0)]), [0.4, 1.6, 0.0]);
    }

    /// The sum is continuous across the picture's radius: a mask at
    /// +200 over the picture's 100% at radius 1, its own radius at 1.00
    /// (the picture's term) and at 1.01 (a term of its own), takes 200%
    /// either way; and past the cap, the fallback onto the picture's
    /// term changes the radius and not the sum.
    #[test]
    fn the_sum_is_continuous_across_the_pictures_radius() {
        let picture = on(1.0, 1.0, 0.01);
        let total = |radius: f32| {
            let locals = [masked(2.0, Some(radius), None)];
            let plan = Plan::finishing(&picture, &locals).unwrap();
            let mut out = vec![0.0; plan.terms()];
            plan.amounts_at(&[(&locals[0].baked, 1.0)], &mut out);
            (plan.terms(), out.iter().sum::<f32>())
        };
        let (at, off) = (total(1.0), total(1.01));
        assert_eq!((at.0, off.0), (1, 2));
        assert!((at.1 - off.1).abs() < 1e-6, "{at:?} against {off:?}");
        assert_eq!(at.1, AMOUNT_MAX);
        // Four own settings: the fourth on the picture's term, the sum
        // the same as were it a term of its own.
        let locals = [
            masked(0.5, Some(0.6), None),
            masked(0.5, Some(1.1), None),
            masked(0.5, Some(2.0), None),
            masked(0.9, Some(2.9), None),
        ];
        let plan = Plan::finishing(&picture, &locals).unwrap();
        assert_eq!(plan.terms(), 4);
        let on: Vec<(&crate::finish::Baked, f32)> =
            locals.iter().map(|l| (&l.baked, 1.0)).collect();
        let mut out = vec![0.0; 4];
        plan.amounts_at(&on, &mut out);
        assert!(
            (out.iter().sum::<f32>() - AMOUNT_MAX).abs() < 1e-6,
            "{out:?}"
        );
    }

    /// The cap's fallback is said once each time it changes, not at
    /// every plan: a draw and a finish of the same edit say it once.
    #[test]
    fn the_caps_fallback_is_said_once_a_change() {
        let said = Dropped(std::sync::Mutex::new(Vec::new()));
        let one = vec![(3, 2.9f32.to_bits(), 0.01f32.to_bits())];
        assert!(!said.say(Vec::new()));
        assert!(said.say(one.clone()));
        assert!(!said.say(one.clone()));
        assert!(!said.say(Vec::new()), "nothing to say when none is dropped");
        assert!(said.say(one), "and again once it comes back");
    }

    /// Three own terms overlapping at their tops, over the picture's at
    /// its top, are held at 200% together, each its share of it.
    #[test]
    fn three_overlapping_own_terms_are_held_at_the_top() {
        let locals = [
            masked(2.0, Some(0.6), None),
            masked(2.0, Some(1.5), Some(0.0)),
            masked(2.0, Some(3.0), Some(0.05)),
        ];
        let plan = Plan::finishing(&on(2.0, 1.0, 0.01), &locals).unwrap();
        let on: Vec<(&crate::finish::Baked, f32)> =
            locals.iter().map(|l| (&l.baked, 1.0)).collect();
        let mut out = vec![0.0; plan.terms()];
        plan.amounts_at(&on, &mut out);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|a| (a - 0.5).abs() < 1e-6), "{out:?}");
        assert!((out.iter().sum::<f32>() - AMOUNT_MAX).abs() < 1e-6);
    }

    /// The terms on a picture: with no own setting, the single term's
    /// sharpen, to the bit; with one, each pixel where only the
    /// picture's term has an amount is what that term makes it, to the
    /// bit, and every pixel is the terms' deltas summed on the encoded
    /// luminance, each against its own radius's blur and its own
    /// threshold; and a band is the whole frame's rows, to the bit.
    #[test]
    fn the_terms_sum_on_the_luminance() {
        let (w, h) = (40, 33);
        let px: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = ((i * 2654435761) % 1000) as f32 / 1000.0;
                [v, v * 0.7 + 0.05, 0.2]
            })
            .collect();
        let picture = on(0.8, 1.3, 0.005);
        let single = Plan::finishing(&picture, &[masked(0.4, None, None)]).unwrap();
        let ones: Vec<f32> = (0..w * h).map(|i| (i % 7) as f32 * 0.2).collect();
        assert_eq!(
            sharpen_plan_rows(&px, w, h, 0, h, &single, Some(&ones)),
            sharpen_rows(&px, w, h, 0, h, &single.picture, Some(&ones))
        );
        let locals = [
            masked(0.6, Some(3.0), Some(0.0)),
            masked(1.0, Some(0.5), None),
        ];
        let plan = Plan::finishing(&picture, &locals).unwrap();
        assert_eq!(plan.terms(), 3);
        // Amounts by pixel: the picture's alone on the left half, the
        // terms each over their own stretches, all three on the right.
        let amounts: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let x = i % w;
                if x < w / 2 {
                    [ones[i], 0.0, 0.0]
                } else {
                    [0.8, (x % 3) as f32 * 0.4, ((x + 1) % 2) as f32 * 1.7]
                }
            })
            .collect();
        let got = sharpen_plan_rows(&px, w, h, 0, h, &plan, Some(&amounts));
        let alone = sharpen_rows(&px, w, h, 0, h, &plan.picture, Some(&ones));
        let l: Vec<f32> = px.iter().map(|c| luminance(*c)).collect();
        let blurred = [
            blur(&l, w, h, 1.3),
            blur(&l, w, h, 0.5),
            blur(&l, w, h, 3.0),
        ];
        let thresholds = [0.005, 0.005, 0.0];
        for i in 0..w * h {
            if i % w < w / 2 {
                assert_eq!(got[i], alone[i], "pixel {i}, the picture's term alone");
                continue;
            }
            let a = &amounts[i * 3..i * 3 + 3];
            let delta: f32 = (0..3)
                .filter(|&k| a[k] > 0.0)
                .map(|k| a[k] * soft(l[i] - blurred[k][i], thresholds[k]))
                .sum();
            let want = moved(px[i], l[i], a.iter().sum(), delta, false);
            for k in 0..3 {
                assert!((got[i][k] - want[k]).abs() < 1e-6, "pixel {i}");
            }
        }
        let n = plan.reach();
        for (y0, count) in [(0, 6), (6, 6), (17, 9), (27, 6)] {
            let lo = y0 - n.min(y0);
            let hi = (y0 + count + n).min(h);
            let band = sharpen_plan_rows(
                &px[lo * w..hi * w],
                w,
                hi - lo,
                y0 - lo,
                count,
                &plan,
                Some(&amounts[y0 * w * 3..(y0 + count) * w * 3]),
            );
            assert_eq!(band, got[y0 * w..(y0 + count) * w], "rows {y0} on");
        }
    }
}
