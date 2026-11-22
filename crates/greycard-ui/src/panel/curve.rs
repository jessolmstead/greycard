use std::path::Path;
use std::rc::Rc;

use greycard_edit::curve::{self, Channel, Curves, Parametric, Point};
use greycard_viewport::render::Tap;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::worker::Worker;
use crate::{App, Shared, finish};

pub(crate) fn current_channel(app: &App) -> Channel {
    Channel::from_name(app.get_curve_channel().as_str()).unwrap_or(Channel::Rgb)
}

/// The CURVES section's mode that shows the parametric curve; the
/// other is "Point".
pub(crate) const PARAMETRIC: &str = "Parametric";

pub(crate) fn parametric_mode(app: &App) -> bool {
    app.get_curve_mode().as_str() == PARAMETRIC
}

/// The panel's parametric curve.
pub(crate) fn panel_parametric(app: &App) -> Parametric {
    let splits: Vec<f32> = app.get_para_splits().iter().collect();
    let mut p = Parametric {
        highlights: app.get_para_highlights(),
        lights: app.get_para_lights(),
        darks: app.get_para_darks(),
        shadows: app.get_para_shadows(),
        ..Default::default()
    };
    if let Ok(s) = <[f32; 3]>::try_from(splits) {
        p.splits = s;
    }
    p.splits = p.ordered_splits();
    p
}

pub(crate) fn set_panel_parametric(app: &App, p: &Parametric) {
    app.set_para_highlights(p.highlights);
    app.set_para_lights(p.lights);
    app.set_para_darks(p.darks);
    app.set_para_shadows(p.shadows);
    app.set_para_splits(ModelRc::new(VecModel::from(p.ordered_splits().to_vec())));
}

/// The panel's points for a channel.
pub(crate) fn curve_points(app: &App, channel: Channel) -> Vec<Point> {
    let flat: Vec<f32> = match channel {
        Channel::Rgb => app.get_curve_rgb().iter().collect(),
        Channel::Red => app.get_curve_red().iter().collect(),
        Channel::Green => app.get_curve_green().iter().collect(),
        Channel::Blue => app.get_curve_blue().iter().collect(),
        Channel::RedGreen => app.get_curve_red_green().iter().collect(),
        Channel::BlueYellow => app.get_curve_blue_yellow().iter().collect(),
    };
    let points: Vec<Point> = flat.as_chunks::<2>().0.to_vec();
    if points.len() < 2 {
        channel.identity()
    } else {
        points
    }
}

/// `points` as the panel holds them: in order of In, so the first is
/// the black point and the last the white, whatever order a sidecar
/// wrote them in; fewer than two, the channel's line that does
/// nothing, as the spline takes them.
pub(crate) fn shown_points(channel: Channel, points: &[Point]) -> Vec<Point> {
    if points.len() < 2 {
        return channel.identity();
    }
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));
    sorted
}

/// A channel's points read off the panel, or `was` itself when they
/// are only `was` as the panel shows it: a sidecar's points out of
/// order are put in order on the way in, and that alone is no edit.
pub(crate) fn unless_unmoved(channel: Channel, read: Vec<Point>, was: &[Point]) -> Vec<Point> {
    if read == shown_points(channel, was) {
        was.to_vec()
    } else {
        read
    }
}

/// Put `points` on the panel for `channel`, in order of In.
pub(crate) fn set_curve_points(app: &App, channel: Channel, points: &[Point]) {
    let flat: Vec<f32> = shown_points(channel, points)
        .iter()
        .flat_map(|p| [p[0], p[1]])
        .collect();
    let model = ModelRc::new(VecModel::from(flat));
    match channel {
        Channel::Rgb => app.set_curve_rgb(model),
        Channel::Red => app.set_curve_red(model),
        Channel::Green => app.set_curve_green(model),
        Channel::Blue => app.set_curve_blue(model),
        Channel::RedGreen => app.set_curve_red_green(model),
        Channel::BlueYellow => app.set_curve_blue_yellow(model),
    }
}

/// Where point `i` of `points` goes when moved to (x, y): every
/// point keeps its order with a gap to its neighbors' In, the ends
/// sliding along their edges, the black point from 0 and the white
/// point to 1, everything past an end taking its Out; Out stays
/// within 0 to 1. A drag and a typed In or Out both land through
/// this.
pub(crate) fn moved_point(points: &[Point], i: usize, x: f32, y: f32) -> Point {
    let last = points.len().saturating_sub(1);
    let lo = if i == 0 {
        0.0
    } else {
        (points[i - 1][0] + curve::MIN_GAP).min(1.0)
    };
    let hi = if i >= last {
        1.0
    } else {
        (points[i + 1][0] - curve::MIN_GAP).max(lo)
    };
    [x.clamp(lo, hi), y.clamp(0.0, 1.0)]
}

/// The point the curve dropper takes for a picked tone `value`, added
/// to `points` (in order of In) when none is near: a tone past an end
/// slid in is that end's, the curve there being its Out; else the
/// nearest point within twice the gap; else a new one on the curve as
/// it is, so the picture holds still until the drag.
pub(crate) fn picked_point(points: &mut Vec<Point>, value: f32) -> usize {
    let last = points.len() - 1;
    if value <= points[0][0] {
        return 0;
    }
    if value >= points[last][0] {
        return last;
    }
    let near = points
        .iter()
        .enumerate()
        .map(|(i, p)| (i, (p[0] - value).abs()))
        .filter(|(_, d)| *d < curve::MIN_GAP * 2.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i);
    near.unwrap_or_else(|| {
        let at = points.partition_point(|p| p[0] < value);
        points.insert(at, [value, curve::evaluate(points, value)]);
        at
    })
}

/// The selected point, if there is one and the current channel has it.
pub(crate) fn selected_point(app: &App) -> Option<usize> {
    let i = usize::try_from(app.get_curve_selected()).ok()?;
    (i < curve_points(app, current_channel(app)).len()).then_some(i)
}

/// The point curve's units in the In and Out fields: 0 to 255.
pub(crate) const FIELD_SCALE: f32 = 255.0;

/// Make `index` the selected point, or none, and show its In and Out.
pub(crate) fn select_point(app: &App, index: Option<usize>) {
    app.set_curve_selected(index.map_or(-1, |i| i as i32));
    show_selected(app);
}

/// The In and Out fields for the selected point, which a move or a
/// typed number has just changed; a selection the channel no longer
/// has is let go.
pub(crate) fn show_selected(app: &App) {
    let points = curve_points(app, current_channel(app));
    match selected_point(app).map(|i| points[i]) {
        Some([x, y]) => {
            app.set_curve_point_in(format!("{}", (x * FIELD_SCALE).round()).into());
            app.set_curve_point_out(format!("{}", (y * FIELD_SCALE).round()).into());
        }
        None => {
            app.set_curve_selected(-1);
            app.set_curve_point_in("".into());
            app.set_curve_point_out("".into());
        }
    }
}

/// The panel's curves for the look it edits, as far as the curve
/// editor's histogram needs them: the switch, the parametric curve,
/// and the tone curves.
pub(crate) fn panel_curves(app: &App) -> Curves {
    let mut curves = Curves {
        enabled: app.get_curves_enabled(),
        parametric: panel_parametric(app),
        ..Curves::default()
    };
    for channel in [Channel::Rgb, Channel::Red, Channel::Green, Channel::Blue] {
        *curves.channel_mut(channel) = curve_points(app, channel);
    }
    curves
}

/// Whether the curve editor is on the panel: the panel up, on a tab
/// that has the CURVES section (Develop, or Masks with a mask
/// chosen), and the section open. Its histogram is taken only then.
pub(crate) fn curves_shown(app: &App) -> bool {
    let tab_has_it = match app.get_shown_tab().as_str() {
        "Develop" => true,
        "Masks" => app.get_target() > 0,
        _ => false,
    };
    tab_has_it && !app.get_collapsed_curves() && !app.get_right_hidden()
}

/// Where in the look the curve editor's histogram is taken for the
/// curve shown: a color curve's past every point curve, the masks'
/// and the global look's; a mask's point curves' before every curve;
/// the global look's past the masks' own.
pub(crate) fn histogram_tap(app: &App, target: Option<usize>) -> Tap {
    if shown_curve(app).is_some_and(|c| c.is_color()) {
        Tap::AfterCurves
    } else if target.is_some() {
        Tap::BeforeCurves
    } else {
        Tap::BeforeGlobalCurves
    }
}

/// The histogram the editor draws behind its curve: the last one read
/// (`develop.bins`), only when it was taken at the tap the curve shown
/// reads ([`histogram_tap`]) and weighed as it is now ([`weighed_by`]).
/// Another tap's is another curve's input, the global editor's behind
/// a mask's, or a point curve's behind a color curve's; another
/// weighing's is another mask's, or a mask's behind the whole frame's;
/// neither is drawn. The one rule for every draw of the editor and the
/// frame loop's.
pub(crate) fn backdrop<'a>(app: &App, develop: &'a crate::owners::Develop) -> Option<&'a [u32]> {
    develop
        .bins
        .as_ref()
        .filter(|(tap, weigh_by, _)| {
            *tap == histogram_tap(app, develop.target) && *weigh_by == weighed_by(app, develop)
        })
        .map(|(_, _, bins)| bins.as_slice())
}

/// The mask the editor's histogram is weighed by, as the frame loop
/// weighs the analyses: the one chosen, when the Selection toggle asks
/// and its mask has something live in it.
fn weighed_by(app: &App, develop: &crate::owners::Develop) -> Option<usize> {
    develop
        .target
        .filter(|&k| {
            k < finish::MAX_LOCALS
                && develop
                    .edit
                    .adjustments
                    .get(k)
                    .is_some_and(|a| !a.mask.is_empty())
        })
        .filter(|_| app.get_scope_selection())
}

/// The curve the editor shows: `None` for the parametric curve, else
/// the point curve's channel.
pub(crate) fn shown_curve(app: &App) -> Option<Channel> {
    (!parametric_mode(app)).then(|| current_channel(app))
}

/// The histogram `curve` takes in, from `bins`, the one before every
/// curve: each channel's counts moved through the curves ahead of it
/// in the order ([`Curves::taken_in`]). A level stands for the span
/// half a level either side of it, as the scopes round a value to its
/// level; its count is spread over the levels that span lands on,
/// in proportion, so a curve that stretches a range leaves no gaps
/// between the levels it spreads them to, and one that squeezes it
/// piles them up. Each tone curve maps one channel to itself, so
/// nothing else is needed. The parametric curve, first, and curves
/// switched off take in the histogram as it is; so do the color
/// curves, whose histogram is taken past every point curve, the
/// masks' and the global look's together (`Tap::AfterCurves`), since
/// those blend by each pixel's weights and no move of one look's
/// curves can stand for them.
pub(crate) fn histogram_taken_in(
    curves: &Curves,
    curve: Option<Channel>,
    bins: &[u32],
) -> Vec<f32> {
    let Some(point) = curve.filter(|c| !c.is_color() && curves.enabled) else {
        return bins.iter().map(|&n| n as f32).collect();
    };
    let mut moved = vec![0.0f32; bins.len()];
    // Where a value lands, in levels.
    let taken = curves.taken_in_by(Some(point));
    let to = |c: usize, e: f32| taken.at(c, e.clamp(0.0, 1.0)).clamp(0.0, 1.0) * 255.0;
    for c in 0..3 {
        for i in 0..256 {
            let n = bins[c * 256 + i] as f32;
            if n == 0.0 {
                continue;
            }
            let (a, b) = (
                to(c, (i as f32 - 0.5) / 255.0),
                to(c, (i as f32 + 0.5) / 255.0),
            );
            let (lo, hi) = (a.min(b), a.max(b));
            let level = |v: f32| ((v + 0.5).floor().max(0.0) as usize).min(255);
            if hi - lo < 1e-3 {
                moved[c * 256 + level((lo + hi) / 2.0)] += n;
                continue;
            }
            for j in level(lo)..=level(hi) {
                let cover = hi.min(j as f32 + 0.5) - lo.max(j as f32 - 0.5);
                if cover > 0.0 {
                    moved[c * 256 + j] += n * cover / (hi - lo);
                }
            }
        }
    }
    moved
}

/// The height the histogram's bars are scaled to: the highest level
/// of `channels` in `inner`, the histogram moved without what clips,
/// leaving out each channel's first and last level, where a curve
/// ahead that clips piles up everything past its end.
pub(crate) fn backdrop_peak(inner: &[f32], channels: &[usize]) -> f32 {
    channels
        .iter()
        .flat_map(|c| inner[c * 256 + 1..c * 256 + 255].iter().copied())
        .fold(1.0f32, f32::max)
}

/// The tone the curve dropper takes for `picked`, where it lands on
/// the editor's backdrop: its own channel's value (or for the master
/// its luminance) moved through the curves ahead of the curve shown,
/// as [`histogram_taken_in`] moves the histogram; for a color curve
/// its lightness past the look's own tone curves.
pub(crate) fn picked_tone(curves: &Curves, channel: Channel, picked: &finish::Picked) -> f32 {
    let at = |c: usize| curves.taken_in(Some(channel), c, picked.encoded[c]);
    match channel {
        Channel::Red => at(0),
        Channel::Green => at(1),
        Channel::Blue => at(2),
        Channel::Rgb => finish::encoded_luma([at(0), at(1), at(2)]),
        Channel::RedGreen | Channel::BlueYellow => picked.lightness,
    }
    .clamp(0.0, 1.0)
}

/// `bins` without the clipped: the first and last level of each
/// channel emptied, so the histogram's height is set by the picture
/// and not by what clips.
fn unclipped(bins: &[u32]) -> Vec<u32> {
    let mut inner = bins.to_vec();
    for c in 0..3 {
        inner[c * 256] = 0;
        inner[c * 256 + 255] = 0;
    }
    inner
}

/// The curve editor's side in physical pixels, and the physical
/// pixels to a logical one: the widget's width as laid out, in
/// physical pixels as the editor reports it (so a move to a screen of
/// another scale draws it again), and the picture lands one to one.
/// Before the editor is first laid out, its side at the panel's
/// default width.
pub(crate) fn curve_raster(app: &App) -> (usize, f32) {
    let scale = app.window().scale_factor().max(0.5);
    let side = match app.get_curve_side() {
        s if s > 0.0 => s,
        _ => 288.0 * scale,
    };
    ((side.round() as usize).clamp(64, 4096), scale)
}

/// The curve editor's picture: a grid, the histogram behind, the
/// diagonal, the channel's curve and its points, at the editor's own
/// size in physical pixels ([`curve_raster`]), y up, every stroke and
/// mark sized in logical pixels so it looks the same at any scale. For
/// a color curve the line that does nothing is the level one through
/// the middle, the ground is tinted toward what each way gives, and
/// the histogram is of lightness. In the parametric mode it is the
/// parametric curve over the master's ground, its splits as lines
/// with a handle at the foot of each, and no points.
///
/// `bins` are the histogram before every curve; each curve is drawn
/// over what it takes in ([`histogram_taken_in`]).
pub(crate) fn draw_curve(app: &App, bins: Option<&[u32]>) -> slint::Image {
    let (n, u) = curve_raster(app);
    let parametric = parametric_mode(app).then(|| panel_parametric(app));
    let channel = if parametric.is_some() {
        Channel::Rgb
    } else {
        current_channel(app)
    };
    let points = curve_points(app, channel);
    let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(n as u32, n as u32);
    let px = buf.make_mut_slice();
    let top = (n - 1) as f32;
    // Blend `rgb` over the pixel at (x, y up) by `alpha`.
    let blend = |px: &mut [slint::Rgb8Pixel], x: usize, y_up: usize, rgb: [u8; 3], alpha: f32| {
        if x < n && y_up < n && alpha > 0.0 {
            let a = alpha.min(1.0);
            let p = &mut px[(n - 1 - y_up) * n + x];
            let mix = |was: u8, to: u8| (was as f32 + (to as f32 - was as f32) * a).round() as u8;
            p.r = mix(p.r, rgb[0]);
            p.g = mix(p.g, rgb[1]);
            p.b = mix(p.b, rgb[2]);
        }
    };
    // A line `width` logical pixels wide at x = `at` (0 to 1) or, with
    // `across`, at that height, its edges anti-aliased.
    let rule = |px: &mut [slint::Rgb8Pixel], at: f32, width: f32, rgb: [u8; 3], across: bool| {
        let (center, half) = (at * top, width * u / 2.0);
        let lo = (center - half - 1.0).floor().max(0.0) as usize;
        let hi = ((center + half + 1.0).ceil() as usize).min(n - 1);
        for k in lo..=hi {
            let a = (half + 0.5 - (k as f32 - center).abs()).clamp(0.0, 1.0);
            for i in 0..n {
                if across {
                    blend(px, i, k, rgb, a);
                } else {
                    blend(px, k, i, rgb, a);
                }
            }
        }
    };
    for p in px.iter_mut() {
        *p = slint::Rgb8Pixel {
            r: 0x14,
            g: 0x14,
            b: 0x14,
        };
    }
    // The tinted ground of a color curve: up is red or yellow, down
    // is green or blue, strongest at the edges.
    if channel.is_color() {
        let (up, down): ([f32; 3], [f32; 3]) = match channel {
            Channel::RedGreen => (
                [0x2e as f32, 0x08 as f32, 0x0c as f32],
                [0.0, 0x24 as f32, 0x08 as f32],
            ),
            _ => (
                [0x2a as f32, 0x22 as f32, 0.0],
                [0x04 as f32, 0x0e as f32, 0x34 as f32],
            ),
        };
        let mid = top / 2.0;
        for y in 0..n {
            let t = (y as f32 - mid) / mid;
            let tint = if t > 0.0 {
                up.map(|v| v * t)
            } else {
                down.map(|v| v * -t)
            };
            for x in 0..n {
                let p = &mut px[(n - 1 - y) * n + x];
                p.r = 0x14 + tint[0] as u8;
                p.g = 0x14 + tint[1] as u8;
                p.b = 0x14 + tint[2] as u8;
            }
        }
    }
    // The histogram, dim, behind everything: of what this curve takes
    // in, its height set by what does not clip.
    if let Some(bins) = bins {
        let curves = panel_curves(app);
        let curve = shown_curve(app);
        let shown = histogram_taken_in(&curves, curve, bins);
        let inner = histogram_taken_in(&curves, curve, &unclipped(bins));
        // A bar at a pixel column `height` of the whole (on a square
        // root scale): the whole pixels it fills, and how much of the
        // one above.
        let bar = |height: f32| {
            let hf = height.clamp(0.0, 1.0).sqrt() * n as f32;
            let h = (hf.floor() as usize).min(n);
            (h, if h < n { hf - h as f32 } else { 0.0 })
        };
        // Between the 256 levels at a column: linearly.
        let level_at = |bins: &[f32], c: usize, e: f32| {
            let v = e.clamp(0.0, 1.0) * 255.0;
            let i = (v as usize).min(254);
            let f = v - i as f32;
            bins[c * 256 + i] * (1.0 - f) + bins[c * 256 + i + 1] * f
        };
        if channel.is_color() {
            // The lightness histogram: the channels' mean, resampled
            // from the encoded axis to Oklab's L, which for a grey is
            // the cube root of its linear value.
            let column = |bins: &[f32]| -> Vec<f32> {
                (0..n)
                    .map(|j| {
                        let l = j as f32 / top;
                        let e = finish::encode(l * l * l);
                        (0..3).map(|c| level_at(bins, c, e)).sum::<f32>() / 3.0
                    })
                    .collect()
            };
            // Not the ends: the black and the white, where a curve
            // ahead that clips piles everything past it.
            let inner = column(&inner);
            let peak = inner[1..n - 1].iter().copied().fold(1.0f32, f32::max);
            for (x, v) in column(&shown).into_iter().enumerate() {
                let (h, top_cover) = bar(v / peak);
                let add = |p: &mut slint::Rgb8Pixel, by: u8| {
                    p.r = p.r.saturating_add(by);
                    p.g = p.g.saturating_add(by);
                    p.b = p.b.saturating_add(by);
                };
                for y in 0..h {
                    add(&mut px[(n - 1 - y) * n + x], 0x1c);
                }
                if top_cover > 0.0 {
                    add(
                        &mut px[(n - 1 - h) * n + x],
                        (0x1c as f32 * top_cover).round() as u8,
                    );
                }
            }
        } else {
            let channels: Vec<(usize, [u8; 3])> = match channel {
                Channel::Rgb => vec![
                    (0, [0x3a, 0x2a, 0x2a]),
                    (1, [0x2a, 0x3a, 0x2a]),
                    (2, [0x2a, 0x2a, 0x3a]),
                ],
                Channel::Red => vec![(0, [0x4a, 0x26, 0x26])],
                Channel::Green => vec![(1, [0x26, 0x4a, 0x26])],
                _ => vec![(2, [0x26, 0x30, 0x4a])],
            };
            let peak = backdrop_peak(
                &inner,
                &channels.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            );
            for x in 0..n {
                let e = x as f32 / top;
                for (c, color) in &channels {
                    let (h, top_cover) = bar(level_at(&shown, *c, e) / peak);
                    for y in 0..h {
                        let p = &mut px[(n - 1 - y) * n + x];
                        p.r = p.r.max(color[0]);
                        p.g = p.g.max(color[1]);
                        p.b = p.b.max(color[2]);
                    }
                    if top_cover > 0.0 {
                        let p = &mut px[(n - 1 - h) * n + x];
                        let lift = |was: u8, to: u8| {
                            was.max(
                                (was as f32 + to.saturating_sub(was) as f32 * top_cover).round()
                                    as u8,
                            )
                        };
                        p.r = lift(p.r, color[0]);
                        p.g = lift(p.g, color[1]);
                        p.b = lift(p.b, color[2]);
                    }
                }
            }
        }
    }
    // Grid at the quarters, a logical pixel wide, and the line that
    // does nothing: the diagonal, or the level line for a color curve.
    for q in [0.25, 0.5, 0.75] {
        rule(px, q, 1.0, [0x24, 0x24, 0x24], false);
        rule(px, q, 1.0, [0x24, 0x24, 0x24], true);
    }
    if channel.is_color() {
        rule(px, 0.5, 1.0, [0x3a, 0x3a, 0x3a], true);
    } else {
        let half = u / 2.0;
        let reach = (half * std::f32::consts::SQRT_2 + 1.5).ceil() as isize;
        for i in 0..n as isize {
            for y in (i - reach).max(0)..=(i + reach).min(n as isize - 1) {
                let d = (y - i).abs() as f32 * std::f32::consts::FRAC_1_SQRT_2;
                blend(
                    px,
                    i as usize,
                    y as usize,
                    [0x2c, 0x2c, 0x2c],
                    (half + 0.5 - d).clamp(0.0, 1.0),
                );
            }
        }
    }
    // The splits of the parametric curve: a line each, under the
    // curve.
    if let Some(p) = &parametric {
        for s in p.ordered_splits() {
            rule(px, s, 1.0, [0x44, 0x44, 0x44], false);
        }
    }
    // The curve, two logical pixels wide: each pixel near it covered
    // by its distance to the nearer of the two segments joining the
    // samples either side.
    let color = match channel {
        Channel::Red => [0xe8, 0x60, 0x60],
        Channel::Green => [0x60, 0xe0, 0x60],
        Channel::Blue => [0x60, 0x98, 0xf0],
        _ => [0xd8, 0xd8, 0xd8],
    };
    let ys: Vec<f32> = (0..n)
        .map(|x| {
            let at = x as f32 / top;
            let y = match &parametric {
                Some(p) => p.at(at),
                None => curve::evaluate(&points, at),
            };
            y * top
        })
        .collect();
    // From (px, py) to the segment from a to b.
    let to_segment = |a: (f32, f32), b: (f32, f32), p: (f32, f32)| {
        let (vx, vy) = (b.0 - a.0, b.1 - a.1);
        let len2 = vx * vx + vy * vy;
        let t = if len2 > 1e-6 {
            (((p.0 - a.0) * vx + (p.1 - a.1) * vy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ((p.0 - a.0 - t * vx).powi(2) + (p.1 - a.1 - t * vy).powi(2)).sqrt()
    };
    let half = u;
    let margin = half + 1.5;
    for x in 0..n {
        let here = (x as f32, ys[x]);
        let before = (x > 0).then(|| (x as f32 - 1.0, ys[x - 1]));
        let after = (x + 1 < n).then(|| (x as f32 + 1.0, ys[x + 1]));
        let (lo, hi) = [before, after]
            .into_iter()
            .flatten()
            .fold((here.1, here.1), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
        let lo = (lo - margin).floor().max(0.0) as usize;
        let hi = ((hi + margin).ceil() as usize).min(n - 1);
        for y in lo..=hi {
            let p = (x as f32, y as f32);
            let d = [before, after]
                .into_iter()
                .flatten()
                .map(|q| to_segment(q, here, p))
                .fold((p.1 - here.1).abs(), f32::min);
            blend(px, x, y, color, (half + 0.5 - d).clamp(0.0, 1.0));
        }
    }
    // A disc of `radius` logical pixels about (cx, cy), its rim from
    // `inner` out in `ring`, white within, both edges anti-aliased.
    let disc = |px: &mut [slint::Rgb8Pixel],
                (cx, cy): (f32, f32),
                radius: f32,
                inner: f32,
                ring: [u8; 3]| {
        let reach = (radius + 1.5).ceil() as isize;
        let (ix, iy) = (cx.round() as isize, cy.round() as isize);
        for y in (iy - reach).max(0)..=(iy + reach).min(n as isize - 1) {
            for x in (ix - reach).max(0)..=(ix + reach).min(n as isize - 1) {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                let outer = (radius + 0.5 - d).clamp(0.0, 1.0);
                if outer > 0.0 {
                    let white = (inner + 0.5 - d).clamp(0.0, 1.0);
                    let mix = |r: u8| (r as f32 + (255.0 - r as f32) * white).round() as u8;
                    blend(px, x as usize, y as usize, ring.map(mix), outer);
                }
            }
        }
    };
    if let Some(p) = &parametric {
        // The split handles: a triangle at the foot of each line, its
        // apex up, eight logical pixels high, with a dark rim.
        let height = 8.0 * u;
        let rim = u;
        for split in p.ordered_splits() {
            let cx = split * top;
            for y in 0..=(height.ceil() as usize).min(n - 1) {
                let half = height - y as f32;
                let lo = (cx - half - 1.0).floor().max(0.0) as usize;
                let hi = ((cx + half + 1.0).ceil() as usize).min(n - 1);
                for x in lo..=hi {
                    let side = half - (x as f32 - cx).abs();
                    let cover = (side + 0.5).clamp(0.0, 1.0);
                    let inside = (side - rim + 0.5).min(y as f32 - rim + 0.5).clamp(0.0, 1.0);
                    let shade = (0x14 as f32 + (0xff - 0x14) as f32 * inside).round() as u8;
                    blend(px, x, y, [shade; 3], cover);
                }
            }
        }
    } else {
        // The points: discs eight logical pixels across with a dark
        // rim; the selected one ten across, ringed in the accent two
        // and a half wide, so two whole physical pixels of it at the
        // least between its anti-aliased edges. Each is drawn whole inside
        // the picture, so an end point at a corner, or slid along an
        // edge, shows its whole handle; the press reaches it all the
        // same (`curve::HIT`).
        let selected = selected_point(app);
        for (i, p) in points.iter().enumerate() {
            let (radius, ring_width, ring) = if selected == Some(i) {
                (5.0 * u, (2.5 * u).max(2.5), [0x6e, 0xa8, 0xff])
            } else {
                (4.0 * u, u.max(1.0), [0x14, 0x14, 0x14])
            };
            let inside = |v: f32| (v * top).clamp(radius, top - radius);
            disc(
                px,
                (inside(p[0]), inside(p[1])),
                radius,
                radius - ring_width,
                ring,
            );
        }
    }
    if let Some(path) = std::env::var_os("GREYCARD_UI_CURVE")
        && let Some(img) = image::RgbImage::from_raw(n as u32, n as u32, buf.as_bytes().to_vec())
        && let Err(e) = img.save(&path)
    {
        tracing::warn!("curve {}: {e}", Path::new(&path).display());
    }
    slint::Image::from_rgb8(buf)
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, _worker: &Rc<Worker>) {
    // The curve editor: press selects or adds a point, drag moves it,
    // release records the edit, a double-click removes a point. On
    // the parametric curve the same presses take a split point
    // instead, and a double-click puts it back.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_press(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if parametric_mode(&app) {
                let p = panel_parametric(&app);
                state.borrow_mut().develop.curve_drag = p.nearest_split(x);
                return;
            }
            let channel = current_channel(&app);
            let mut points = curve_points(&app, channel);
            let index = match curve::nearest_point(&points, x, y) {
                Some(i) => i,
                None => {
                    let x = x.clamp(0.0, 1.0);
                    // Not on top of another point's x, nor past an
                    // end slid in, where the curve is that end's Out
                    // and a point would be a new end: a press that
                    // neither takes a point nor adds one lets the
                    // selected one go.
                    let (first, last) = (points[0][0], points[points.len() - 1][0]);
                    if x < first
                        || x > last
                        || points.iter().any(|p| (p[0] - x).abs() < curve::MIN_GAP)
                    {
                        select_point(&app, None);
                        app.set_curve_image(draw_curve(
                            &app,
                            backdrop(&app, &state.borrow().develop),
                        ));
                        return;
                    }
                    let at = points.partition_point(|p| p[0] < x);
                    points.insert(at, [x, y.clamp(0.0, 1.0)]);
                    at
                }
            };
            state.borrow_mut().develop.curve_drag = Some(index);
            set_curve_points(&app, channel, &points);
            // Pressed is selected, and stays so after the release.
            select_point(&app, Some(index));
            app.set_curve_image(draw_curve(&app, backdrop(&app, &state.borrow().develop)));
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_move(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(i) = state.borrow().develop.curve_drag else {
                return;
            };
            if parametric_mode(&app) {
                let mut p = panel_parametric(&app);
                if i < p.splits.len() {
                    p.set_split(i, x);
                    set_panel_parametric(&app, &p);
                    app.set_curve_image(draw_curve(&app, backdrop(&app, &state.borrow().develop)));
                    app.window().request_redraw();
                }
                return;
            }
            let channel = current_channel(&app);
            let mut points = curve_points(&app, channel);
            if i >= points.len() {
                return;
            }
            points[i] = moved_point(&points, i, x, y);
            set_curve_points(&app, channel, &points);
            show_selected(&app);
            app.set_curve_image(draw_curve(&app, backdrop(&app, &state.borrow().develop)));
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_release(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if state.borrow_mut().develop.curve_drag.take().is_some() {
                app.invoke_view_changed();
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_double(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if parametric_mode(&app) {
                let mut p = panel_parametric(&app);
                let Some(i) = p.nearest_split(x) else {
                    return;
                };
                p.set_split(i, Parametric::DEFAULT_SPLITS[i]);
                state.borrow_mut().develop.curve_drag = None;
                set_panel_parametric(&app, &p);
                app.set_curve_image(draw_curve(&app, backdrop(&app, &state.borrow().develop)));
                app.invoke_view_changed();
                return;
            }
            let channel = current_channel(&app);
            let mut points = curve_points(&app, channel);
            let Some(i) = curve::nearest_point(&points, x, y) else {
                return;
            };
            if i == 0 || i == points.len() - 1 {
                return;
            }
            points.remove(i);
            state.borrow_mut().develop.curve_drag = None;
            set_curve_points(&app, channel, &points);
            select_point(&app, None);
            app.set_curve_image(draw_curve(&app, backdrop(&app, &state.borrow().develop)));
            app.invoke_view_changed();
        });
    }
    // A number typed into the selected point's In or Out, 0 to 255:
    // the point moves there as a drag would take it, between its
    // neighbors, and the move is an edit as a drag's release is.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_typed(move |which, typed| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if parametric_mode(&app) {
                return;
            }
            let Some(i) = selected_point(&app) else {
                return;
            };
            let Some(v) = crate::entry::parse(typed.as_str(), FIELD_SCALE, 0.0, 1.0) else {
                return;
            };
            let channel = current_channel(&app);
            let mut points = curve_points(&app, channel);
            let [x, y] = points[i];
            let (x, y) = if which.as_str() == "in" {
                (v, y)
            } else {
                (x, v)
            };
            points[i] = moved_point(&points, i, x, y);
            set_curve_points(&app, channel, &points);
            show_selected(&app);
            app.set_curve_image(draw_curve(&app, backdrop(&app, &state.borrow().develop)));
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if parametric_mode(&app) {
                set_panel_parametric(&app, &Parametric::default());
            } else {
                let channel = current_channel(&app);
                set_curve_points(&app, channel, &channel.identity());
            }
            select_point(&app, None);
            app.set_curve_image(draw_curve(&app, backdrop(&app, &state.borrow().develop)));
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_channel_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            select_point(&app, None);
            app.set_curve_image(draw_curve(&app, backdrop(&app, &state.borrow().develop)));
        });
    }
    // The other curve: the same picture, drawn for it. The curve
    // dropper is a point-curve tool and its button hides with the
    // points, so one in hand is put down. A slider of the parametric
    // curve is an edit like any other, and its picture follows at
    // once rather than with the next histogram.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_mode_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if parametric_mode(&app) && app.get_picking() == "Curve" {
                app.invoke_pick_started("Curve".into());
            }
            select_point(&app, None);
            app.set_curve_image(draw_curve(&app, backdrop(&app, &state.borrow().develop)));
        });
    }
    // The editor laid out at another width (the panel dragged, the
    // section shown): its picture again at the new size. The editor's
    // first layout can come while the state is held (a tab chosen from
    // a handler that holds it), so the picture then waits a turn of
    // the event loop.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_resized(move || {
            let redraw = {
                let (state, app_weak) = (state.clone(), app_weak.clone());
                move || {
                    let Some(app) = app_weak.upgrade() else {
                        return false;
                    };
                    let Ok(st) = state.try_borrow() else {
                        return false;
                    };
                    app.set_curve_image(draw_curve(&app, backdrop(&app, &st.develop)));
                    true
                }
            };
            if !redraw() {
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    redraw();
                });
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_parametric_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_curve_image(draw_curve(&app, backdrop(&app, &state.borrow().develop)));
            app.invoke_view_changed();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// The point fields' ids.
    const IN: &str = "CurvesSection::point-in-field";
    const OUT: &str = "CurvesSection::point-out-field";

    /// A point moved, by a drag or a typed number, keeps between its
    /// neighbors' In with a gap to each; the ends slide along their
    /// edges, kept a gap from their one neighbor and inside 0 to 1;
    /// Out keeps to 0 to 1.
    #[test]
    fn a_moved_point_keeps_between_its_neighbors() {
        let points = [[0.0, 0.0], [0.25, 0.3], [0.5, 0.5], [1.0, 1.0]];
        assert_eq!(moved_point(&points, 2, 0.6, 0.7), [0.6, 0.7]);
        assert_eq!(
            moved_point(&points, 2, 0.1, 0.5),
            [0.25 + curve::MIN_GAP, 0.5]
        );
        assert_eq!(
            moved_point(&points, 2, 1.5, 0.5),
            [1.0 - curve::MIN_GAP, 0.5]
        );
        assert_eq!(moved_point(&points, 1, 0.0, 0.3), [curve::MIN_GAP, 0.3]);
        assert_eq!(moved_point(&points, 1, 0.3, 2.0), [0.3, 1.0]);
        assert_eq!(moved_point(&points, 1, 0.3, -1.0), [0.3, 0.0]);
        // The black point slides right along the bottom, up to a gap
        // from its neighbor, and no further left than 0.
        assert_eq!(moved_point(&points, 0, 0.1, 0.0), [0.1, 0.0]);
        assert_eq!(
            moved_point(&points, 0, 0.4, 0.2),
            [0.25 - curve::MIN_GAP, 0.2]
        );
        assert_eq!(moved_point(&points, 0, -0.3, -0.5), [0.0, 0.0]);
        // The white point slides left along the top, down to a gap
        // from its neighbor, and no further right than 1.
        assert_eq!(moved_point(&points, 3, 0.8, 1.0), [0.8, 1.0]);
        assert_eq!(
            moved_point(&points, 3, 0.4, 0.8),
            [0.5 + curve::MIN_GAP, 0.8]
        );
        assert_eq!(moved_point(&points, 3, 1.5, 1.4), [1.0, 1.0]);
        // With nothing between them, each keeps a gap from the other.
        let two = [[0.0, 0.0], [1.0, 1.0]];
        assert_eq!(moved_point(&two, 0, 1.2, 0.0), [1.0 - curve::MIN_GAP, 0.0]);
        assert_eq!(moved_point(&two, 1, -1.0, 1.0), [curve::MIN_GAP, 1.0]);
        // A sidecar's ends nearer than the gap: kept in the box, no
        // panic.
        let close = [[0.0, 0.0], [0.004, 0.5], [1.0, 1.0]];
        assert_eq!(moved_point(&close, 0, 0.5, 0.1), [0.0, 0.1]);
    }

    /// The black point dragged right along the bottom sets where black
    /// begins, and its In reads it; the white point's In typed sets
    /// where white begins, kept a gap from the black point. A press
    /// past an end adds no point there; a double-click does not remove
    /// an end; Reset puts both back at the corners.
    #[test]
    fn the_end_points_slide_along_their_edges() {
        use crate::testing;
        use slint::platform::Key;

        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_curve_mode("Point".into());
        app.set_curve_channel("RGB".into());
        let changes = Rc::new(Cell::new(0));
        {
            let changes = changes.clone();
            app.on_view_changed(move || changes.set(changes.get() + 1));
        }
        let points = || curve_points(&app, Channel::Rgb);
        let type_into = |id: &str, text: &str| {
            let (at, size) = testing::by_id(&app, id);
            testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
            testing::type_text(&app, text);
            testing::press(&app, Key::Return);
        };

        // The black point taken and dragged right along the bottom.
        app.invoke_curve_press(0.0, 0.0);
        assert_eq!(app.get_curve_selected(), 0);
        assert_eq!(app.get_curve_point_in(), "0");
        app.invoke_curve_move(0.2, 0.0);
        app.invoke_curve_release();
        assert_eq!(points(), vec![[0.2, 0.0], [1.0, 1.0]]);
        assert_eq!(app.get_curve_point_in(), "51");
        assert_eq!(app.get_curve_point_out(), "0");
        assert_eq!(changes.get(), 1, "the drag is one change");
        assert_eq!(curve::evaluate(&points(), 0.1), 0.0, "under it is black");

        // A press past the black point adds nothing and lets go.
        app.invoke_curve_press(0.1, 0.5);
        app.invoke_curve_release();
        assert_eq!(points().len(), 2);
        assert_eq!(app.get_curve_selected(), -1);

        // The white point's In typed: it moves left along the top,
        // and no nearer the black point than the gap.
        app.invoke_curve_press(1.0, 1.0);
        app.invoke_curve_release();
        assert_eq!(app.get_curve_selected(), 1);
        type_into(IN, "200");
        assert_eq!(points()[1], [200.0 / 255.0, 1.0]);
        assert_eq!(app.get_curve_point_in(), "200");
        type_into(IN, "10");
        assert_eq!(points()[1][0], 0.2 + curve::MIN_GAP);
        assert_eq!(app.get_curve_point_in(), "54");
        type_into(IN, "200");
        // The drag, the press on the white point, three typed.
        assert_eq!(changes.get(), 5);

        // A press past the white point adds nothing and lets go.
        app.invoke_curve_press(0.9, 0.5);
        app.invoke_curve_release();
        assert_eq!(points().len(), 2);
        assert_eq!(app.get_curve_selected(), -1);
        assert_eq!(changes.get(), 5);

        // A double-click on either end removes neither.
        app.invoke_curve_double(0.2, 0.0);
        app.invoke_curve_double(200.0 / 255.0, 1.0);
        assert_eq!(points(), vec![[0.2, 0.0], [200.0 / 255.0, 1.0]]);
        assert_eq!(changes.get(), 5);

        // Reset: the corners again.
        app.invoke_curve_reset();
        assert_eq!(points(), curve::identity());
        assert_eq!(app.get_curve_selected(), -1);
    }

    /// The dropper on a tone past an end slid in takes that end and
    /// adds nothing; past the white point with a point just inside
    /// it, the white point still; within reach of two points, the
    /// nearer; near none, a new one on the curve.
    #[test]
    fn the_dropper_takes_an_end_for_a_tone_past_it() {
        let mut points = vec![[0.2, 0.0], [0.5, 0.5], [1.0, 1.0]];
        assert_eq!(picked_point(&mut points, 0.1), 0);
        assert_eq!(picked_point(&mut points, 0.0), 0);
        assert_eq!(points.len(), 3);

        let mut points = vec![[0.0, 0.0], [0.785, 0.7], [0.8, 1.0]];
        assert_eq!(picked_point(&mut points, 0.95), 2);
        assert_eq!(picked_point(&mut points, 0.795), 2, "the nearer");
        assert_eq!(picked_point(&mut points, 0.79), 1, "the nearer");
        assert_eq!(points.len(), 3);

        let y = curve::evaluate(&points, 0.4);
        assert_eq!(picked_point(&mut points, 0.4), 1);
        assert_eq!(points, vec![[0.0, 0.0], [0.4, y], [0.785, 0.7], [0.8, 1.0]]);
    }

    /// A sidecar's points out of order: the panel holds them in order,
    /// so the black point is the first and a press between the ends
    /// adds a point; read back unmoved they are the sidecar's as
    /// written, so showing them is no edit; a point added is.
    #[test]
    fn a_curve_out_of_order_is_shown_in_order_and_is_no_edit() {
        use crate::panel::edit::{read_edit, show_edit};
        use crate::testing;

        let app = testing::window(1);
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_curve_mode("Point".into());
        app.set_curve_channel("RGB".into());
        let mut edit = state.borrow().develop.edit.clone();
        edit.curves.rgb = vec![[1.0, 1.0], [0.5, 0.6], [0.0, 0.0]];
        show_edit(&state.borrow(), &edit, &app, None);
        let points = || curve_points(&app, Channel::Rgb);
        assert_eq!(points(), vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]);
        assert_eq!(read_edit(&app, &edit, None), edit, "showing it is no edit");

        app.invoke_curve_press(0.25, 0.3);
        app.invoke_curve_release();
        assert_eq!(points().len(), 4);
        assert_eq!(points()[1], [0.25, 0.3]);
        assert_eq!(
            read_edit(&app, &edit, None).curves.rgb,
            vec![[0.0, 0.0], [0.25, 0.3], [0.5, 0.6], [1.0, 1.0]]
        );
    }

    /// A point pressed is selected and stays so after the release;
    /// its In and Out typed in the window move it, clamped between
    /// its neighbors, and each is one change; it is still selected
    /// after. Escape changes nothing; a press that takes no point and
    /// adds none, or a point deleted, lets the selection go.
    #[test]
    fn a_selected_point_takes_a_typed_in_and_out() {
        use crate::testing;
        use slint::platform::Key;

        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_curve_mode("Point".into());
        app.set_curve_channel("RGB".into());
        let changes = Rc::new(Cell::new(0));
        {
            let changes = changes.clone();
            app.on_view_changed(move || changes.set(changes.get() + 1));
        }
        let points = || curve_points(&app, Channel::Rgb);
        let type_into = |id: &str, text: &str, key: Key| {
            let (at, size) = testing::by_id(&app, id);
            testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
            testing::type_text(&app, text);
            testing::press(&app, key);
        };
        assert_eq!(app.get_curve_selected(), -1);

        app.invoke_curve_press(0.5, 0.5);
        app.invoke_curve_release();
        assert_eq!(points().len(), 3);
        assert_eq!(
            app.get_curve_selected(),
            1,
            "stays selected after the release"
        );
        assert_eq!(app.get_curve_point_in(), "128");
        assert_eq!(app.get_curve_point_out(), "128");
        let after_press = changes.get();

        // Enter on the reading as it was: 128 is not 0.5, and reading
        // it back would move the point. Nothing happens.
        let (at, size) = testing::by_id(&app, IN);
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        testing::press(&app, Key::Return);
        assert_eq!(points()[1], [0.5, 0.5]);
        assert_eq!(changes.get(), after_press);

        type_into(OUT, "200", Key::Return);
        assert_eq!(points()[1], [0.5, 200.0 / 255.0]);
        assert_eq!(app.get_curve_selected(), 1, "still selected");
        assert_eq!(app.get_curve_point_out(), "200");
        assert_eq!(changes.get(), after_press + 1, "one change");

        // In past the left neighbor: kept a gap from it.
        type_into(IN, "0", Key::Return);
        assert_eq!(points()[1][0], curve::MIN_GAP);
        assert_eq!(app.get_curve_point_in(), "3");
        assert_eq!(app.get_curve_selected(), 1);
        assert_eq!(changes.get(), after_press + 2);

        // Escape and a non-number: nothing.
        type_into(IN, "100", Key::Escape);
        type_into(OUT, "abc", Key::Return);
        assert_eq!(points()[1], [curve::MIN_GAP, 200.0 / 255.0]);
        assert_eq!(changes.get(), after_press + 2);

        // A field open, then a click on a channel, which takes no
        // focus: the field closes, and a key typed after is the
        // window's, not the field's.
        let meta = Rc::new(Cell::new(0));
        {
            let meta = meta.clone();
            app.on_meta_key(move |k| {
                if k == "5" {
                    meta.set(meta.get() + 1);
                }
                k == "5"
            });
        }
        let (at, size) = testing::by_id(&app, OUT);
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        let row = "CurvesSection::channel-row";
        testing::by_id(&app, row);
        let (at, size) = testing::segment(&app, row, "Blue");
        testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(app.get_curve_channel(), "Blue");
        testing::press(&app, "5");
        testing::press(&app, Key::Return);
        assert_eq!(meta.get(), 1, "the key reached the window");
        assert_eq!(changes.get(), after_press + 2);
        assert_eq!(app.get_curve_selected(), -1, "a channel changed lets go");
        app.set_curve_channel("RGB".into());
        app.invoke_curve_channel_changed();
        assert_eq!(points()[1], [curve::MIN_GAP, 200.0 / 255.0]);

        // A press on nothing it can add a point at: none selected.
        app.invoke_curve_press(0.005, 0.9);
        app.invoke_curve_release();
        assert_eq!(points().len(), 3);
        assert_eq!(app.get_curve_selected(), -1);
        assert_eq!(app.get_curve_point_in(), "");
        // Selected again, then deleted: none selected.
        app.invoke_curve_press(points()[1][0], points()[1][1]);
        app.invoke_curve_release();
        assert_eq!(app.get_curve_selected(), 1);
        app.invoke_curve_double(points()[1][0], points()[1][1]);
        assert_eq!(points().len(), 2);
        assert_eq!(app.get_curve_selected(), -1);
    }

    fn image_bytes(img: &slint::Image) -> Vec<u8> {
        img.to_rgba8()
            .map(|b| b.as_bytes().to_vec())
            .unwrap_or_default()
    }

    /// A histogram with something in every bin, as the analysis reads
    /// one back: red, green and blue, 256 bins each.
    fn some_bins(lift: u32) -> Vec<u32> {
        (0..768).map(|i| (i % 256) as u32 * lift + 5).collect()
    }

    /// A window on a picture with one mask, of `shape`, and the last
    /// analysis's histogram in hand, a mask's (taken before every
    /// curve); the Masks tab showing, nothing chosen yet.
    fn a_mask_of(shape: &str, bins: &[u32]) -> (App, Rc<Shared>, Rc<crate::worker::Worker>) {
        use crate::testing;
        let app = testing::window(1);
        let (state, worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.develop.source_size = (1200, 800);
            st.develop.edit.adjustments.push(greycard_edit::Adjustment {
                id: 1,
                name: "Mask 1".into(),
                enabled: true,
                mask: greycard_edit::mask::Mask {
                    components: vec![greycard_edit::mask::Component {
                        shape: greycard_edit::mask::Shape::of_kind(shape),
                        ..Default::default()
                    }],
                    invert: false,
                    ..Default::default()
                },
                look: Default::default(),
                opacity: 1.0,
            });
            st.develop.bins = Some((Tap::BeforeCurves, None, bins.to_vec()));
        }
        app.set_panel_tab("Masks".into());
        app.invoke_tab_changed("Masks".into());
        app.set_curve_mode("Point".into());
        app.set_curve_channel("RGB".into());
        (app, state, worker)
    }

    /// A mask chosen and its Curves opened: the editor has the last
    /// histogram behind it at once, with no slider or point touched.
    /// Choosing the mask redraws the editor for the mask's curve while
    /// the state is held, and the histogram was read through a second
    /// borrow that found nothing, so the editor stood bare until the
    /// next analysis, which a switch of target alone does not bring.
    /// Back on the global look, that histogram is a mask's input, not
    /// the global curve's, and the editor stands bare until its own.
    #[test]
    fn a_masks_curve_editor_has_its_histogram_when_shown() {
        let frame = some_bins(3);
        let (app, state, _worker) = a_mask_of("Radial", &frame);
        let bare = image_bytes(&draw_curve(&app, None));
        let drawn = image_bytes(&draw_curve(&app, Some(&frame)));
        assert_ne!(bare, drawn, "the histogram draws");

        app.invoke_target_changed(1);
        app.set_collapsed_curves(false);
        assert_eq!(state.borrow().develop.target, Some(0));
        assert_eq!(app.get_target(), 1);
        assert!(!app.get_collapsed_curves());
        assert_eq!(
            image_bytes(&app.get_curve_image()),
            drawn,
            "the mask's curve editor stood without its histogram"
        );

        // The global curve, by the Develop tab: the mask's histogram
        // is not its input.
        app.set_panel_tab("Develop".into());
        app.invoke_tab_changed("Develop".into());
        assert_eq!(state.borrow().develop.target, None);
        assert_eq!(
            image_bytes(&app.get_curve_image()),
            bare,
            "the global curve drawn over the mask's histogram"
        );
    }

    /// Bins of another tap are never drawn, whichever way the editor
    /// comes to another curve: the global look's histogram behind a
    /// mask's curve once the mask is chosen, or a point curve's behind
    /// R/G or B/Y, or theirs behind a point curve, by the channel or
    /// the mode. Back where they were taken, they draw again.
    #[test]
    fn bins_of_another_tap_are_never_drawn_behind_the_curve() {
        let frame = some_bins(3);
        let (app, state, _worker) = a_mask_of("Radial", &frame);
        app.set_panel_tab("Develop".into());
        app.invoke_tab_changed("Develop".into());
        state.borrow_mut().develop.bins = Some((Tap::BeforeGlobalCurves, None, frame.clone()));
        app.invoke_curve_channel_changed();
        let shown = || image_bytes(&app.get_curve_image());
        let drawn = image_bytes(&draw_curve(&app, Some(&frame)));
        assert_eq!(shown(), drawn, "the global curve over its own histogram");

        for color in ["R/G", "B/Y"] {
            app.set_curve_channel(color.into());
            app.invoke_curve_channel_changed();
            assert_eq!(
                shown(),
                image_bytes(&draw_curve(&app, None)),
                "{color} drawn over the point curves' input"
            );
        }
        app.set_curve_channel("RGB".into());
        app.invoke_curve_channel_changed();
        assert_eq!(shown(), drawn, "back on the master");
        state.borrow_mut().develop.bins = Some((Tap::AfterCurves, None, frame.clone()));
        app.invoke_curve_mode_changed();
        assert_eq!(
            shown(),
            image_bytes(&draw_curve(&app, None)),
            "the master drawn over the color curves' input"
        );

        // The global look's histogram, and a mask chosen.
        state.borrow_mut().develop.bins = Some((Tap::BeforeGlobalCurves, None, frame.clone()));
        app.set_panel_tab("Masks".into());
        app.invoke_tab_changed("Masks".into());
        app.invoke_target_changed(1);
        app.set_collapsed_curves(false);
        assert_eq!(state.borrow().develop.target, Some(0));
        assert_eq!(
            shown(),
            image_bytes(&draw_curve(&app, None)),
            "the mask's curve drawn over the global curve's input"
        );
    }

    /// Weighed by the selection, the editor's histogram is the chosen
    /// mask's: one weighed by another mask is not drawn behind the
    /// curve, nor one weighed once the toggle no longer weighs. Back
    /// where it was taken, it draws again.
    #[test]
    fn bins_of_another_weighing_are_never_drawn_behind_the_curve() {
        // A mask of a kind, not a control found by its words.
        let radial = "Radial";
        let frame = some_bins(3);
        let (app, state, _worker) = a_mask_of(radial, &frame);
        {
            let mut st = state.borrow_mut();
            let mut second = st.develop.edit.adjustments[0].clone();
            second.id = 2;
            second.name = "Mask 2".into();
            st.develop.edit.adjustments.push(second);
        }
        app.set_scope_selection(true);
        app.invoke_target_changed(1);
        app.set_collapsed_curves(false);
        // The analysis weighed by the first mask lands.
        let weighed = some_bins(1);
        crate::panel::startup::bins_read(
            &mut state.borrow_mut(),
            &app,
            Tap::BeforeCurves,
            Some(0),
            weighed.clone(),
        );
        let shown = || image_bytes(&app.get_curve_image());
        let drawn = image_bytes(&draw_curve(&app, Some(&weighed)));
        assert_eq!(shown(), drawn, "the first mask's own histogram");

        app.invoke_target_changed(2);
        assert_eq!(state.borrow().develop.target, Some(1));
        assert_eq!(
            shown(),
            image_bytes(&draw_curve(&app, None)),
            "the second mask's curve drawn over the first's weighing"
        );

        app.invoke_target_changed(1);
        assert_eq!(shown(), drawn, "back on the first mask");
        app.set_scope_selection(false);
        assert_eq!(
            backdrop(&app, &state.borrow().develop),
            None,
            "the whole frame's input drawn as the mask's weighing"
        );
    }

    /// While the Original shows, the histogram read is the Original's
    /// picture's, not what the edit's curves take in: the editor keeps
    /// the edit's.
    #[test]
    fn the_originals_histogram_is_never_drawn_behind_the_curve() {
        // A mask of a kind, not a control found by its words.
        let radial = "Radial";
        let frame = some_bins(3);
        let (app, state, _worker) = a_mask_of(radial, &frame);
        app.set_panel_tab("Develop".into());
        app.invoke_tab_changed("Develop".into());
        app.set_collapsed_curves(false);
        crate::panel::startup::bins_read(
            &mut state.borrow_mut(),
            &app,
            Tap::BeforeGlobalCurves,
            None,
            frame.clone(),
        );
        let drawn = image_bytes(&draw_curve(&app, Some(&frame)));
        assert_eq!(image_bytes(&app.get_curve_image()), drawn);

        {
            let mut st = state.borrow_mut();
            st.develop.original.on = true;
            st.develop.original.edit = Some(greycard_edit::Edit::default());
            assert!(crate::panel::original::shown(&st).is_some());
        }
        crate::panel::startup::bins_read(
            &mut state.borrow_mut(),
            &app,
            Tap::BeforeGlobalCurves,
            None,
            some_bins(1),
        );
        assert_eq!(
            image_bytes(&app.get_curve_image()),
            drawn,
            "the edit's curve drawn over the Original's histogram"
        );
        assert_eq!(
            backdrop(&app, &state.borrow().develop),
            Some(frame.as_slice())
        );
    }

    /// A learned mask still being made when its curve editor is shown:
    /// the editor has the picture's histogram meanwhile, the raster's
    /// landing does not take it away, the landing changes the locals
    /// the analysis is keyed on (so a new one is read with no touch),
    /// and that read is what the editor then shows.
    #[test]
    fn a_masks_curve_editor_follows_its_raster_landing() {
        use crate::panel::mask::bake_locals;
        use std::sync::Arc;

        let frame = some_bins(3);
        let (app, state, _worker) = a_mask_of("Sky", &frame);
        app.invoke_target_changed(1);
        app.set_collapsed_curves(false);
        let drawn = image_bytes(&draw_curve(&app, Some(&frame)));
        assert_eq!(image_bytes(&app.get_curve_image()), drawn);

        let locals = |state: &Rc<Shared>| {
            let mut st = state.borrow_mut();
            let st = &mut *st;
            bake_locals(
                &st.develop.edit,
                &mut st.develop.rasters,
                &mut st.develop.learned,
                2.0 / 3.0,
                None,
            )
            .0
        };
        let waiting = locals(&state);
        assert!(waiting[0].rasters[0].is_none(), "the sky is not made yet");

        crate::panel::deliver::deliver(
            &app,
            crate::worker::Outcome::Mask {
                key: (1, 0),
                shape: greycard_edit::mask::Shape::of_kind("Sky"),
                file: None,
                raster: Arc::new(greycard_edit::brush::Raster::from_data(
                    1.0,
                    4,
                    vec![255; 16],
                )),
                provider: Some("CPU"),
                seconds: 0.1,
                note: None,
                part: None,
            },
        );
        assert_eq!(
            image_bytes(&app.get_curve_image()),
            drawn,
            "the raster's landing took the histogram away"
        );
        let landed = locals(&state);
        assert!(landed[0].rasters[0].is_some());
        assert_ne!(waiting, landed, "the analysis would not be read again");

        // The analysis read again, weighed by the sky now there.
        let weighed = some_bins(1);
        crate::panel::startup::bins_read(
            &mut state.borrow_mut(),
            &app,
            Tap::BeforeCurves,
            None,
            weighed.clone(),
        );
        assert_eq!(
            image_bytes(&app.get_curve_image()),
            image_bytes(&draw_curve(&app, Some(&weighed)))
        );
        assert_eq!(
            backdrop(&app, &state.borrow().develop),
            Some(weighed.as_slice())
        );
    }

    /// A histogram of one level a channel: red at `r`, green at `g`,
    /// blue at `b`, `n` pixels each.
    fn spikes(r: usize, g: usize, b: usize, n: u32) -> Vec<u32> {
        let mut bins = vec![0; 768];
        bins[r] += n;
        bins[256 + g] += n;
        bins[512 + b] += n;
        bins
    }

    /// Curves with a parametric curve and a red curve that both move
    /// the middle.
    fn lifted() -> Curves {
        Curves {
            parametric: Parametric {
                darks: 0.6,
                lights: 0.6,
                ..Default::default()
            },
            red: vec![[0.0, 0.0], [0.5, 0.8], [1.0, 1.0]],
            ..Default::default()
        }
    }

    /// The level a channel's counts sit at on the whole: their mean.
    fn mean_level(bins: &[f32], c: usize) -> f32 {
        let row = &bins[c * 256..c * 256 + 256];
        let total: f32 = row.iter().sum();
        row.iter()
            .enumerate()
            .map(|(i, n)| i as f32 * n)
            .sum::<f32>()
            / total
    }

    /// Each curve is drawn over what it takes in: the parametric over
    /// the histogram before every curve; a channel's curve over it
    /// moved by the parametric; the master over it moved by the
    /// parametric and each channel's own curve; the color curves over
    /// it moved by the master as well. Every count is kept, moved to
    /// where the curves before put its level.
    #[test]
    fn each_curve_is_drawn_over_what_it_takes_in() {
        let curves = lifted();
        let before = spikes(128, 128, 128, 7);
        let as_is: Vec<f32> = before.iter().map(|&n| n as f32).collect();
        let at = |curve| histogram_taken_in(&curves, curve, &before);
        assert_eq!(at(None), as_is, "the parametric is first");
        let mid = 128.0 / 255.0;
        let p = curves.parametric.at(mid) * 255.0;
        assert!(p > 132.0, "the parametric lifts the middle: {p}");
        let near =
            |a: f32, b: f32, what: &str| assert!((a - b).abs() < 1.0, "{what}: {a} against {b}");
        for c in [Channel::Red, Channel::Green, Channel::Blue] {
            let shown = at(Some(c));
            let total: f32 = shown.iter().sum();
            assert!((total - 21.0).abs() < 1e-3, "{c:?}: {total} counts");
            for k in 0..3 {
                near(mean_level(&shown, k), p, &format!("{c:?}, channel {k}"));
            }
        }
        let master = at(Some(Channel::Rgb));
        let red = curve::evaluate(&curves.red, p / 255.0) * 255.0;
        near(mean_level(&master, 0), red, "red past its own curve");
        near(mean_level(&master, 1), p, "green's own is the line");
        assert!(mean_level(&master, 0) > mean_level(&master, 1) + 10.0);
        // The color curves' histogram is taken past every point curve
        // already, the masks' with the global look's: it is drawn as
        // it comes.
        assert_eq!(at(Some(Channel::RedGreen)), as_is);
        assert_eq!(at(Some(Channel::BlueYellow)), as_is);
        // Switched off, every curve is over the histogram as it is.
        let off = Curves {
            enabled: false,
            ..lifted()
        };
        for curve in [
            None,
            Some(Channel::Rgb),
            Some(Channel::Red),
            Some(Channel::BlueYellow),
        ] {
            assert_eq!(histogram_taken_in(&off, curve, &before), as_is);
        }
    }

    /// A curve ahead that stretches a range spreads each level over
    /// the levels it lands on, so the histogram behind the next has no
    /// gaps where the stretch is: every level between where the first
    /// and the last of a full histogram land holds something.
    #[test]
    fn a_stretch_ahead_leaves_no_gaps() {
        let curves = lifted();
        let full = vec![100u32; 768];
        let shown = histogram_taken_in(&curves, Some(Channel::Red), &full);
        let to = |v: f32| (curves.parametric.at(v) * 255.0).round() as usize;
        let (lo, hi) = (to(2.0 / 255.0), to(253.0 / 255.0));
        assert!(hi - lo > 200);
        let empty: Vec<usize> = (lo..=hi).filter(|&j| shown[j] <= 0.0).collect();
        assert!(empty.is_empty(), "gaps at {empty:?}");
        let total: f32 = shown[..256].iter().sum();
        assert!((total - 25_600.0).abs() < 0.5, "{total} counts");
    }

    /// The editor's histogram follows the curve shown: a parametric
    /// curve set moves the point curve's backdrop, and a red curve set
    /// moves the master's but not the parametric's.
    #[test]
    fn the_editor_draws_the_histogram_its_curve_takes_in() {
        use crate::testing;
        let app = testing::window(1);
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        let before = some_bins(2);
        let draw = |mode: &str, channel: &str| {
            app.set_curve_mode(mode.into());
            app.set_curve_channel(channel.into());
            image_bytes(&draw_curve(&app, Some(&before)))
        };
        let plain_red = draw("Point", "Red");
        let plain_parametric = draw(PARAMETRIC, "RGB");
        set_panel_parametric(&app, &lifted().parametric);
        assert_ne!(
            draw("Point", "Red"),
            plain_red,
            "the red curve's ground moved"
        );
        set_panel_parametric(&app, &Parametric::default());
        let plain_master = draw("Point", "RGB");
        set_curve_points(&app, Channel::Red, &lifted().red);
        assert_ne!(
            draw("Point", "RGB"),
            plain_master,
            "the master's ground moved"
        );
        assert_eq!(
            draw(PARAMETRIC, "RGB"),
            plain_parametric,
            "the parametric's did not"
        );
    }

    /// The editor's picture is drawn at the editor's own size: its
    /// width as laid out at the window's scale, a pixel of the picture
    /// to a pixel of the screen, and drawn again at a new width.
    #[test]
    fn the_editor_is_drawn_at_its_own_size() {
        use crate::testing;
        let app = testing::window(1);
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.set_curve_side(300.0);
        let picture = draw_curve(&app, Some(&some_bins(1)));
        let size = picture.size();
        assert_eq!((size.width, size.height), (300, 300));
        app.set_curve_side(410.0);
        app.invoke_curve_resized();
        let size = app.get_curve_image().size();
        assert_eq!((size.width, size.height), (410, 410));
    }

    /// The editor reports its width in physical pixels: on a screen of
    /// another scale, the same panel reports the new size, and its
    /// picture is drawn again at it.
    #[test]
    fn a_scale_change_draws_the_editor_again_at_its_new_size() {
        use crate::testing;
        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());
        app.set_collapsed_curves(false);
        testing::settle(&app);
        let before = app.get_curve_side();
        assert!(before > 64.0, "laid out at {before}");
        app.window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
        testing::settle(&app);
        let after = app.get_curve_side();
        assert!(
            (after - 2.0 * before).abs() < 2.0,
            "{before} physical pixels at 1x, {after} at 2x"
        );
        let size = app.get_curve_image().size();
        assert_eq!(
            size.width,
            after.round() as u32,
            "drawn again at the new size"
        );
    }

    /// A curve ahead that clips piles everything past its end on the
    /// black or the white: the bars keep their height, set by the
    /// levels between. A red curve with its black point slid to 0.3
    /// over a flat histogram, under the master: red's middle bars stand
    /// near green's and blue's, not a tenth of them.
    #[test]
    fn a_curve_ahead_that_clips_leaves_the_bars_their_height() {
        let curves = Curves {
            red: vec![[0.3, 0.0], [1.0, 1.0]],
            ..Default::default()
        };
        let flat = vec![100u32; 768];
        let shown = histogram_taken_in(&curves, Some(Channel::Rgb), &flat);
        let inner = histogram_taken_in(&curves, Some(Channel::Rgb), &unclipped(&flat));
        assert!(inner[0] > 5_000.0, "the black point piles up the shadows");
        let peak = backdrop_peak(&inner, &[0, 1, 2]);
        let mid = shown[128] / peak;
        assert!(mid > 0.6, "red's middle at {mid} of the peak");
        assert!((shown[256 + 128] / peak - 1.0).abs() < 0.05, "green's");
    }

    /// The curve dropper lands where the pixel's tone stands in the
    /// backdrop: on a channel's curve, its value past the parametric;
    /// on the master, the luminance of its values past the parametric
    /// and each channel's own curve. A histogram of that one pixel,
    /// moved as the editor moves it, sits where the dropper puts its
    /// point.
    #[test]
    fn the_dropper_lands_on_its_tone_in_the_backdrop() {
        let curves = lifted();
        let encoded = [0.5, 0.4, 0.3];
        let picked = finish::Picked {
            hue: 0.0,
            encoded,
            luma: finish::encoded_luma(encoded),
            lightness: 0.5,
            shown_hue: 0.0,
            shown_chroma: 0.0,
        };
        let level = |v: f32| (v * 255.0).round() as usize;
        let one = spikes(level(0.5), level(0.4), level(0.3), 1);
        for (c, channel) in [Channel::Red, Channel::Green, Channel::Blue]
            .into_iter()
            .enumerate()
        {
            let tone = picked_tone(&curves, channel, &picked);
            let moved = histogram_taken_in(&curves, Some(channel), &one);
            let at = mean_level(&moved, c);
            assert!(
                (tone * 255.0 - at).abs() < 1.0,
                "{channel:?}: {tone} against the spike at {at}"
            );
            assert!(
                (tone - encoded[c]).abs() > 0.02,
                "{channel:?}: the curves ahead moved it"
            );
        }
        let tone = picked_tone(&curves, Channel::Rgb, &picked);
        let moved = histogram_taken_in(&curves, Some(Channel::Rgb), &one);
        let spikes = [0, 1, 2].map(|c| mean_level(&moved, c) / 255.0);
        let expected = finish::encoded_luma(spikes);
        assert!(
            (tone - expected).abs() < 0.01,
            "the master: {tone} against {expected}"
        );
        assert!(
            (tone - picked.luma).abs() > 0.02,
            "the curves ahead moved it"
        );
    }

    /// The editor's histogram is taken where its curve reads: a color
    /// curve's past every point curve, a mask's point curves before
    /// every curve, the global look's past the masks'.
    #[test]
    fn the_histogram_is_taken_where_the_curve_shown_reads() {
        use crate::testing;
        let app = testing::window(1);
        app.set_curve_mode("Point".into());
        app.set_curve_channel("RGB".into());
        assert_eq!(histogram_tap(&app, None), Tap::BeforeGlobalCurves);
        assert_eq!(histogram_tap(&app, Some(0)), Tap::BeforeCurves);
        app.set_curve_mode(PARAMETRIC.into());
        assert_eq!(histogram_tap(&app, Some(0)), Tap::BeforeCurves);
        app.set_curve_mode("Point".into());
        for color in ["R/G", "B/Y"] {
            app.set_curve_channel(color.into());
            assert_eq!(histogram_tap(&app, None), Tap::AfterCurves);
            assert_eq!(histogram_tap(&app, Some(0)), Tap::AfterCurves);
        }
    }

    /// The points are discs: the black and white points' whole handles
    /// at the corners, white within a dark rim, and the selected point
    /// ringed in the accent at least two pixels wide.
    #[test]
    fn the_points_show_whole_and_the_selected_ring_is_two_pixels() {
        use crate::testing;
        let app = testing::window(1);
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.set_curve_side(256.0);
        app.set_curve_mode("Point".into());
        app.set_curve_channel("RGB".into());
        set_curve_points(&app, Channel::Rgb, &[[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]);
        select_point(&app, Some(1));
        let picture = draw_curve(&app, None).to_rgba8().unwrap();
        let (n, scale) = curve_raster(&app);
        let at = |x: usize, y_up: usize| {
            let p = picture.as_slice()[(n - 1 - y_up) * n + x];
            [p.r, p.g, p.b]
        };
        let radius = (4.0 * scale).round() as usize;
        // The black point's center, a radius in from the corner, is
        // white; so is the white point's.
        assert_eq!(at(radius, radius), [0xff; 3], "the black point's handle");
        assert_eq!(
            at(n - 1 - radius, n - 1 - radius),
            [0xff; 3],
            "the white point's"
        );
        // Across the selected point from its center out: white, then
        // the accent, two pixels or more of it all but whole (its edges
        // are anti-aliased into the white and the ground), then not.
        let c = (0.5 * (n - 1) as f32).round() as usize;
        let accent = (0..(8.0 * scale) as usize)
            .filter(|&dx| {
                let [r, _, b] = at(c + dx, c);
                b >= 0xe0 && r <= 0x90
            })
            .count();
        assert_eq!(at(c, c), [0xff; 3], "the selected point's middle");
        assert!(accent >= 2, "{accent} pixels of the accent ring");
    }

    /// The editor's histogram is taken only while the CURVES section
    /// shows: open, on the Develop tab or on Masks with a mask chosen,
    /// the panel up.
    #[test]
    fn the_curves_histogram_is_taken_while_the_section_shows() {
        use crate::testing;
        let app = testing::window(1);
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());
        app.set_collapsed_curves(false);
        assert!(curves_shown(&app));
        app.set_collapsed_curves(true);
        assert!(!curves_shown(&app), "folded");
        app.set_collapsed_curves(false);
        app.set_right_hidden(true);
        assert!(!curves_shown(&app), "the panel put away");
        app.set_right_hidden(false);
        app.set_panel_tab("Masks".into());
        app.set_target(0);
        assert!(!curves_shown(&app), "Masks with no mask chosen");
        app.set_target(1);
        assert!(curves_shown(&app), "a mask's curves");
        app.set_panel_tab("Crop".into());
        assert!(!curves_shown(&app), "Crop");
    }

    /// The clipping lamps read the scopes' own bins, at the export's
    /// end; the curve editor's histogram, taken before the curves,
    /// leaves them alone.
    #[test]
    fn the_clipping_lamps_read_the_scopes_bins_not_the_editors() {
        use crate::testing;
        let app = testing::window(1);
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        crate::panel::startup::clipping_read(&app, &spikes(0, 128, 255, 5));
        assert!(app.get_clip_shadows_lit() && app.get_clip_highlights_lit());
        crate::panel::startup::bins_read(
            &mut state.borrow_mut(),
            &app,
            Tap::BeforeGlobalCurves,
            None,
            spikes(128, 128, 128, 5),
        );
        assert!(
            app.get_clip_shadows_lit() && app.get_clip_highlights_lit(),
            "the editor's histogram put the lamps out"
        );
    }
}
