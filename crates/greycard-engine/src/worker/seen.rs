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
//! is that picture's already while a slider moves. A view moved before
//! the settle lands is drawn from the first round's picture until it
//! does, unless it needs more iterations than that ran (a zoom in),
//! which has a first round of its own ([`Quick::wants_more`]); a save
//! waiting on the settle has it made before the frame is left or the
//! editor closes.

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

    /// Whether `seen` needs more iterations than this sharpened (a zoom
    /// in past them) over a part of the picture, which a first round of
    /// its own is made for. A view of every tile (enlarging a window at
    /// a fit view, a zoom below where the view covers the picture) does
    /// not: that round would be the whole sharpen, which the settle is.
    /// Nor does a view of tiles this did not sharpen at no more
    /// iterations (a pan, a zoom out): the settle covers it. Each such round runs
    /// more iterations than the last, never more than the edit's, so a
    /// move has no more of them than there are counts from the floor to
    /// the edit's (`iterations_seen_at` gives eight to twenty at the
    /// default), and in practice one or two.
    pub(super) fn wants_more(&self, seen: &Seen) -> bool {
        let (part, iterations) = self.needs(seen);
        part.is_some() && iterations > self.iterations
    }
}

/// The rectangle of the tiles `quick` sharpened, pixels of its picture:
/// left, top, right, bottom, the far ends exclusive; none for every tile.
pub(super) fn sharpened_rect(quick: &Quick) -> Option<[u32; 4]> {
    let part = quick.part?;
    let (w, h) = quick.size;
    let tile = sharpen::tile_for(h);
    let (cols, bands) = part.tiles(w, h, tile);
    let at = |v: usize, side: usize| (v * tile).min(side) as u32;
    Some([
        at(cols.start, w),
        at(bands.start, h),
        at(cols.end, w),
        at(bands.end, h),
    ])
}

/// `onto`, a first round's picture of the tiles `quick` sharpened, the
/// rest of it unsharpened, given `from`'s pixels outside those tiles:
/// the picture on screen before it, so a zoom in's round leaves what an
/// earlier round sharpened as it was. Neither is a picture at rest, so
/// what this composes is only ever shown, never kept. Nothing for
/// pictures of other sizes or a round of every tile.
pub(super) fn keep_outside(
    ctx: &greycard_gpu::Context,
    from: &greycard_gpu::wgpu::Texture,
    onto: &greycard_gpu::wgpu::Texture,
    quick: &Quick,
) {
    let (w, h) = (onto.width(), onto.height());
    let Some(rect) = sharpened_rect(quick) else {
        return;
    };
    if (from.width(), from.height()) != (w, h) || quick.size != (w as usize, h as usize) {
        return;
    }
    let mut encoder = ctx.device().create_command_encoder(&Default::default());
    for [l, t, r, b] in outside(rect, (w, h)) {
        let at = |texture| greycard_gpu::wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: greycard_gpu::wgpu::Origin3d { x: l, y: t, z: 0 },
            aspect: greycard_gpu::wgpu::TextureAspect::All,
        };
        encoder.copy_texture_to_texture(
            at(from),
            at(onto),
            greycard_gpu::wgpu::Extent3d {
                width: r - l,
                height: b - t,
                depth_or_array_layers: 1,
            },
        );
    }
    ctx.queue().submit(Some(encoder.finish()));
}

/// The rectangles of a `w` by `h` picture outside `rect`: the rows above
/// and below it whole, and the two sides of it, none empty.
fn outside([x0, y0, x1, y1]: [u32; 4], (w, h): (u32, u32)) -> Vec<[u32; 4]> {
    [
        [0, 0, w, y0],
        [0, y1, w, h],
        [0, y0, x0, y1],
        [x1, y0, w, y1],
    ]
    .into_iter()
    .filter(|[l, t, r, b]| l < r && t < b)
    .collect()
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

    /// A first round asks for another only for a view of part of the
    /// picture that needs more iterations than it ran: 1:1 needs every
    /// iteration after a fit view's eight, and a view further in still
    /// after a 0.4 view's; a view of the whole picture at 0.4 (a window
    /// made larger at a fit view) does not, the settle being its round;
    /// nor a pan past its tiles, a view inside them, or a zoom out.
    #[test]
    fn a_first_round_wants_more_only_for_more_iterations() {
        let size = (8256, 5504);
        let steps = list(SharpenOptions::default());
        let fit = Seen {
            rect: [0.0, 0.0, 1.0, 1.0],
            zoom: 0.26,
        };
        let close = Seen {
            rect: [0.25, 0.25, 0.5, 0.5],
            zoom: 1.0,
        };
        let (_, at_fit) = quickened(&steps, &fit, size, Some(0.74)).expect("fewer");
        assert_eq!(at_fit.iterations, 8);
        assert!(at_fit.wants_more(&close), "1:1 needs every iteration");
        assert!(!at_fit.wants_more(&fit));
        assert!(
            !at_fit.wants_more(&Seen { zoom: 0.15, ..fit }),
            "further out"
        );
        let larger = Seen { zoom: 0.4, ..fit };
        assert!(!at_fit.wants_more(&larger), "the whole picture: the settle");
        let nearer = Seen {
            rect: [0.2, 0.2, 0.8, 0.8],
            zoom: 0.4,
        };
        assert!(at_fit.wants_more(&nearer), "0.4 needs eleven");
        let (_, at_nearer) = quickened(&steps, &nearer, size, Some(0.74)).expect("fewer");
        assert_eq!(at_nearer.iterations, 11);
        assert!(at_nearer.wants_more(&close));
        let (_, at_close) = quickened(&steps, &close, size, Some(0.74)).expect("a part");
        let panned = Seen {
            rect: [0.5, 0.25, 0.75, 0.5],
            zoom: 1.0,
        };
        let inside = Seen {
            rect: [0.3, 0.3, 0.45, 0.45],
            zoom: 2.0,
        };
        for seen in [close, panned, inside, fit] {
            assert!(!at_close.wants_more(&seen), "{seen:?}");
        }
    }

    /// What lies outside a rectangle: every pixel once, none inside.
    #[test]
    fn outside_a_rectangle_is_every_other_pixel_once() {
        let (w, h) = (40u32, 30u32);
        for rect in [
            [10, 5, 30, 20],
            [0, 0, 40, 30],
            [0, 0, 10, 30],
            [35, 25, 40, 30],
        ] {
            let mut seen = vec![0u8; (w * h) as usize];
            for [l, t, r, b] in outside(rect, (w, h)) {
                for y in t..b {
                    for x in l..r {
                        seen[(y * w + x) as usize] += 1;
                    }
                }
            }
            for y in 0..h {
                for x in 0..w {
                    let inside = x >= rect[0] && x < rect[2] && y >= rect[1] && y < rect[3];
                    assert_eq!(
                        seen[(y * w + x) as usize],
                        u8::from(!inside),
                        "{rect:?} {x},{y}"
                    );
                }
            }
        }
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
