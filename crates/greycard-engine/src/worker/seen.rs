//! What the window shows of the open frame, and a develop's first
//! round sharpened only as far as that shows.
//!
//! The capture sharpen's twenty iterations over the whole frame are
//! most of a Clarity or a Dehaze move on a large frame. The window's
//! develop sharpens first only the tiles on screen, at the iterations
//! its zoom can see (`greycard_core::develop::sharpen::Part` and
//! `iterations_seen_at`), and delivers that; the round after it, the
//! settle, sharpens the whole frame and delivers again, giving way to
//! a newer develop between its submissions. The picture at rest is the
//! whole develop's to the bit, and a tile on screen at 1:1 or closer
//! is that picture's already while a slider moves. A view moved past
//! what the first round sharpened before the settle lands (a pan, a
//! zoom, a turn) has a first round of what it shows first
//! ([`Quick::shows`]); a save waiting on the settle has it made before
//! the frame is left or the editor closes.

use greycard_core::develop::sharpen::{self, Part, Radius};
use greycard_edit::Edit;

use crate::stages::{Stage, Step, Steps};

/// What the window shows of the open frame's developed picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seen {
    /// The rectangle on screen as fractions of the picture's width and
    /// height, at the turn the window draws it: left, top, right, and
    /// bottom.
    pub rect: [f32; 4],
    /// View pixels a picture pixel, physical pixels.
    pub zoom: f32,
}

/// How far past what is on screen the part reaches, picture pixels,
/// besides a view pixel's footprint: the view's minify reads about a
/// pixel past it, the output sharpen and the peaking a few more.
const MARGIN: f32 = 32.0;

/// Whether the first round of a develop of `edit` may sharpen only what
/// is seen. Not with a fill to make, whose rounds after the first walk
/// it again and say how far they have got: those rounds sharpen the
/// whole frame as they always have.
pub(super) fn may_quicken(edit: &Edit) -> bool {
    edit.highlight_fill.is_none() && edit.retouch.patches.iter().all(|p| p.method.sourced())
}

/// What a first round sharpened of a picture of `size` whose point
/// spread is `sigma`: the tiles of `part` (every tile for none) at
/// `iterations` of the `full` the edit asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Quick {
    pub size: (usize, usize),
    pub sigma: f32,
    pub full: usize,
    pub part: Option<Part>,
    pub iterations: usize,
}

impl Quick {
    /// The tiles and the iterations `seen` needs of the same picture.
    fn needs(&self, seen: &Seen) -> (Option<Part>, usize) {
        let (w, h) = self.size;
        let part = part_of(seen, self.size).filter(|p| !p.covers(w, h, sharpen::tile_for(h)));
        let iterations = sharpen::iterations_seen_at(seen.zoom, self.sigma, self.full);
        (part, iterations)
    }

    /// Whether the picture this sharpened shows all `seen` shows as the
    /// whole sharpen would: each tile seen among those sharpened, at no
    /// fewer iterations than the view needs.
    pub(super) fn shows(&self, seen: &Seen) -> bool {
        let (part, iterations) = self.needs(seen);
        let (w, h) = self.size;
        let tile = sharpen::tile_for(h);
        let within = |want: &Part, have: &Part| {
            let ((wc, wb), (hc, hb)) = (want.tiles(w, h, tile), have.tiles(w, h, tile));
            wc.is_empty()
                || wb.is_empty()
                || (hc.start <= wc.start
                    && wc.end <= hc.end
                    && hb.start <= wb.start
                    && wb.end <= hb.end)
        };
        iterations <= self.iterations
            && match (&part, &self.part) {
                (_, None) => true,
                (None, Some(_)) => false,
                (Some(want), Some(have)) => within(want, have),
            }
    }
}

/// `steps` with its sharpen made the one `seen` needs of a picture of
/// `size` whose point spread measured `measured`: the tiles on screen,
/// at the iterations its zoom shows, and what that sharpens; none when
/// that is the whole sharpen, or the list has none.
pub(super) fn quickened(
    steps: &Steps,
    seen: &Seen,
    size: (usize, usize),
    measured: Option<f32>,
) -> Option<(Steps, Quick)> {
    let mut quick = None;
    let mut out = steps.clone();
    for step in &mut out.steps {
        let Step::Op(Stage::Sharpen(options, _)) = step else {
            continue;
        };
        if options.part.is_some() {
            continue;
        }
        let sigma = match options.radius {
            Radius::Fixed(r) => r,
            Radius::Auto => measured.unwrap_or(sharpen::DEFAULT_RADIUS),
        };
        let mut q = Quick {
            size,
            sigma,
            full: options.iterations,
            part: None,
            iterations: options.iterations,
        };
        let (part, iterations) = q.needs(seen);
        if part.is_none() && iterations == options.iterations {
            continue;
        }
        (q.part, q.iterations) = (part, iterations);
        options.iterations = iterations;
        options.part = part;
        quick = Some(q);
    }
    quick.map(|q| (out, q))
}

/// The part of a `size` picture `seen` shows, with the margin: none for
/// a rectangle that is not one.
pub(super) fn part_of(seen: &Seen, (w, h): (usize, usize)) -> Option<Part> {
    let [l, t, r, b] = seen.rect;
    if !(l.is_finite() && t.is_finite() && r.is_finite() && b.is_finite()) || r <= l || b <= t {
        return None;
    }
    let reach = MARGIN
        + if seen.zoom > 0.0 && seen.zoom.is_finite() {
            1.0 / seen.zoom
        } else {
            0.0
        };
    let at = |v: f32, side: usize| (v * side as f32).clamp(0.0, side as f32);
    let x0 = (at(l, w) - reach).max(0.0).floor() as usize;
    let y0 = (at(t, h) - reach).max(0.0).floor() as usize;
    let x1 = ((at(r, w) + reach).ceil() as usize).min(w);
    let y1 = ((at(b, h) + reach).ceil() as usize).min(h);
    Some(Part { x0, y0, x1, y1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::develop::sharpen::SharpenOptions;

    use crate::stages::Delta;

    fn list(options: SharpenOptions) -> Steps {
        Steps {
            steps: vec![
                Step::Op(Stage::Dehaze(
                    greycard_core::develop::dehaze::DehazeOptions { amount: 0.3 },
                )),
                Step::Op(Stage::Sharpen(options, Delta::No)),
            ],
            skipped: Vec::new(),
        }
    }

    fn sharpen_of(steps: &Steps) -> SharpenOptions {
        steps
            .ops()
            .find_map(|s| match s {
                Stage::Sharpen(o, _) => Some(*o),
                _ => None,
            })
            .expect("a sharpen")
    }

    /// At 1:1 on part of a frame: the tiles on screen with the margin,
    /// every iteration; at a fit view: every tile, fewer iterations;
    /// at 1:1 over the whole of a small frame: the whole sharpen, and
    /// the list as it was. The rest of the list is untouched.
    #[test]
    fn a_view_sharpens_what_it_shows() {
        let size = (8256, 5504);
        let steps = list(SharpenOptions::default());
        let close = Seen {
            rect: [0.25, 0.5, 0.5, 0.75],
            zoom: 1.0,
        };
        let (quick, _) = quickened(&steps, &close, size, Some(0.74)).expect("a part");
        let o = sharpen_of(&quick);
        assert_eq!(o.iterations, 20);
        assert_eq!(
            o.part,
            Some(Part {
                x0: 2064 - 33,
                y0: 2752 - 33,
                x1: 4128 + 33,
                y1: 4128 + 33,
            })
        );
        assert_eq!(quick.steps[0], steps.steps[0]);
        let fit = Seen {
            rect: [0.0, 0.0, 1.0, 1.0],
            zoom: 0.26,
        };
        let o = sharpen_of(&quickened(&steps, &fit, size, Some(0.74)).expect("fewer").0);
        assert_eq!((o.iterations, o.part), (8, None));
        let all = Seen {
            rect: [0.0, 0.0, 1.0, 1.0],
            zoom: 1.0,
        };
        assert!(quickened(&steps, &all, (600, 400), Some(0.74)).is_none());
        // No sharpen, nothing to do.
        let none = Steps {
            steps: steps.steps[..1].to_vec(),
            skipped: Vec::new(),
        };
        assert!(quickened(&none, &close, size, None).is_none());
        // A rectangle that is not one is the whole frame's tiles.
        let odd = Seen {
            rect: [0.5, 0.5, 0.5, 0.9],
            zoom: 2.0,
        };
        assert!(quickened(&steps, &odd, size, None).is_none());
    }

    /// The point spread the iterations are read at: the edit's fixed
    /// one, the measured one, or the default.
    #[test]
    fn the_iterations_follow_the_point_spread() {
        let size = (8256, 5504);
        let fit = Seen {
            rect: [0.0, 0.0, 1.0, 1.0],
            zoom: 0.2,
        };
        let at = |options: SharpenOptions, measured| {
            sharpen_of(
                &quickened(&list(options), &fit, size, measured)
                    .expect("fewer")
                    .0,
            )
            .iterations
        };
        let wide = SharpenOptions {
            radius: Radius::Fixed(1.5),
            ..Default::default()
        };
        assert_eq!(at(SharpenOptions::default(), Some(0.7)), 8);
        assert_eq!(at(SharpenOptions::default(), None), 8);
        assert!(at(wide, Some(0.7)) > 8);
    }

    /// A first round's picture shows a view inside the tiles it
    /// sharpened at its iterations or fewer; a pan past them, a zoom
    /// that needs more iterations, or a part where every tile was
    /// sharpened does not.
    #[test]
    fn a_first_round_shows_what_its_tiles_hold() {
        let size = (8256, 5504);
        let steps = list(SharpenOptions::default());
        let close = Seen {
            rect: [0.25, 0.25, 0.5, 0.5],
            zoom: 1.0,
        };
        let (_, quick) = quickened(&steps, &close, size, Some(0.74)).expect("a part");
        assert!(quick.shows(&close));
        let inside = Seen {
            rect: [0.3, 0.3, 0.45, 0.45],
            zoom: 2.0,
        };
        assert!(quick.shows(&inside));
        let panned = Seen {
            rect: [0.5, 0.25, 0.75, 0.5],
            zoom: 1.0,
        };
        assert!(!quick.shows(&panned));
        let out = Seen {
            rect: [0.0, 0.0, 1.0, 1.0],
            zoom: 0.25,
        };
        assert!(!quick.shows(&out), "every tile, though fewer iterations");
        let (_, fit) = quickened(&steps, &out, size, Some(0.74)).expect("fewer");
        assert!(fit.shows(&out));
        assert!(!fit.shows(&close), "1:1 needs every iteration");
        let further = Seen { zoom: 0.15, ..out };
        assert!(fit.shows(&further));
    }

    /// A fill to make keeps the whole sharpen in the first round.
    #[test]
    fn a_fill_keeps_the_whole_sharpen() {
        let mut edit = Edit::default();
        assert!(may_quicken(&edit));
        edit.retouch.patches.push(greycard_edit::retouch::Patch {
            method: greycard_edit::retouch::Method::Fill,
            ..Default::default()
        });
        assert!(!may_quicken(&edit));
        edit.retouch.patches[0].method = greycard_edit::retouch::Method::Heal;
        assert!(may_quicken(&edit));
        edit.highlight_fill = Some("x".into());
        assert!(!may_quicken(&edit));
    }
}
