//! Hot and dead photosites, repaired on the mosaic.
//!
//! A photosite that reads far above every same-color neighbor is a
//! defect, not a picture: the demosaic would spread it into a colored
//! dot, and neither denoiser removes it, since nothing around it agrees
//! with it. The rule follows darktable's `hotpixels.c` (copyright
//! 2011-2024 darktable developers, GPL-3.0-or-later): compare with the
//! same-color neighbors two photosites away and replace a hot one
//! with the brightest of them, which the reference notes gives the
//! fewest artifacts when the pixel was not hot after all. Two things
//! are ours: the eight neighbors rather than four, and the threshold,
//! which is not a fixed level but a number of standard deviations of
//! the frame's measured noise at the neighbors' level, so the same
//! setting means the same thing at ISO 100 and ISO 32000.
//!
//! The same-color test alone cannot be left on. Anything at most two
//! photosites across passes it: a star, a specular glint, the
//! catchlight in an eye (a portrait in the test set lost the color of
//! its catchlight to it), a bright speck of texture. Ours adds a second
//! test, on the other three positions of the pattern: their sites
//! adjacent to the candidate must sit at their own local background.
//! A defect is one photosite, and its neighbors of the other colors
//! read what the scene put there; anything the lens drew is spread over
//! several sites by the optics and lights every color round it. With
//! that test the repair is on by default. A site it misses (a defect
//! on a busy background) is the per-camera defect map's to take.
//!
//! **X-Trans.** The reference's X-Trans path builds, for each site of
//! the 6x6 pattern, a table of the four nearest same-color sites by a
//! search outward, since they sit at no one offset. Ours builds its
//! tables from the pattern the same way, once a call
//! ([`super::sites`]), with what the Bayer rule reads carried over: the
//! same-color test reads every site of a green candidate's color in its
//! 5x5 (twelve to sixteen, the adjacent greens of a 2x2 block among
//! them) and of a red or blue one's in its 7x7 (eleven), and the others
//! test reads each of the two other colors, its sites in the 3x3 as
//! the adjacent ones and its sites two and three out as the
//! background. Every 3x3 of X-Trans holds all three colors, so each
//! other color has an adjacent site at every candidate. Red and blue
//! read the 7x7 because their 5x5 holds five, to Bayer's eight: a speck
//! of texture clears five far more often, and on a texture of 2x2 cells
//! the repair took fourteen times Bayer's rate of sites with the 5x5,
//! and a seventh of it with the 7x7.

use rayon::prelude::*;

use super::noise::NoiseModel;
pub use super::rcd::is_bayer;
use super::sites::{Phases, of_color};
use crate::error::{Error, Result};
use crate::raw::CfaPattern;

/// Whether the repair reads this pattern: 2x2 Bayer or X-Trans.
pub fn repairs(pattern: &CfaPattern) -> bool {
    is_bayer(pattern) || pattern.is_xtrans()
}

/// How far out a photosite has to be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HotPixelOptions {
    /// Standard deviations of the noise beyond the brightest (or below
    /// the darkest) same-color neighbor. This is what keeps noise
    /// from counting at high ISO.
    pub sigmas: f32,
    /// The factor a photosite must exceed its brightest same-color
    /// neighbor by (a dead one, fall below its darkest by). This is
    /// what keeps fine texture from counting at base ISO, where the
    /// noise is a fraction of a percent and detail is not.
    pub ratio: f32,
    /// The factor the other colors' adjacent photosites may stand
    /// above their own background (for a dead site, below it) with the
    /// site still a defect. A defect is one photosite; anything the
    /// lens drew, a glint or a star or a catchlight, lights every
    /// color round it. Infinite leaves the test out.
    pub others: f32,
}

impl Default for HotPixelOptions {
    fn default() -> Self {
        Self {
            sigmas: DEFAULT_SIGMAS,
            ratio: DEFAULT_RATIO,
            others: DEFAULT_OTHERS,
        }
    }
}

/// Six sigmas: the noise's tail never gets there over a frame, on the
/// samples here, and a defect is far beyond it.
pub const DEFAULT_SIGMAS: f32 = 6.0;
/// The reference tests at four thirds against four neighbors. Against
/// eight and with the sigma test beside it, three is where real frames
/// keep a few dozen candidates, and even those include catchlights and
/// specks of texture; the others test takes those out, and lower is
/// for frames known to be full of defects.
pub const DEFAULT_RATIO: f32 = 3.0;

/// One and a half: a catchlight, a lit lantern's point or a glint on
/// a phone puts the other colors' adjacent sites at several times
/// their background, a defect leaves them at it. On ten frames
/// rendered with and without the repair, every site this lets through
/// at the default ratio was a lone colored dot, and the sites flagged
/// in several frames of one body (defects for certain) mostly read
/// at background.
pub const DEFAULT_OTHERS: f32 = 1.5;
/// The noise half of the others test: a neighbor counts as lit only
/// past this many sigmas above its background as well, so the noise
/// in a dark patch, where any ratio is easy, does not light it. Lower
/// than [`DEFAULT_SIGMAS`] on purpose: an error here spares a defect,
/// an error the other way recolors a highlight.
pub const OTHERS_SIGMAS: f32 = 3.0;

/// The least noise either test assumes, in units of the range from
/// black to white: two ten-thousandths, about three units of a 14-bit
/// raw, which is a clean sensor's read noise at base ISO. The noise
/// estimate cannot see the read noise where a frame's shadows are
/// clamped at black and often puts it at zero; without this floor a
/// site three units above clamped neighbors would count as hot (on a
/// one-second frame of a dark room, half the repairs were that).
pub const MIN_SIGMA: f32 = 2e-4;

/// What the repair did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotPixelStats {
    /// Photosites above all their neighbors, replaced by the brightest.
    pub hot: usize,
    /// Photosites below all their neighbors, replaced by the darkest.
    pub dead: usize,
}

/// One of the other positions of the 2x2 pattern round a candidate
/// (on X-Trans, one of the other two colors), as the candidate's
/// nearest sites of it read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Neighbors {
    /// The brightest of the adjacent sites of this position (for a
    /// dead candidate, the darkest).
    pub near: f32,
    /// The median of this position's sites two and three photosites
    /// out: the local background they are judged against.
    pub background: f32,
    /// The noise at the background's level.
    pub sigma: f32,
}

/// A photosite the same-color tests flag, and how the other colors
/// round it read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    /// Index into the samples.
    pub index: usize,
    /// Above its same-color neighbors (true) or below them.
    pub hot: bool,
    /// The site as shot.
    pub value: f32,
    /// The brightest (or darkest) same-color neighbor, its repair.
    pub replacement: f32,
    /// On Bayer the horizontal, vertical and diagonal neighbors'
    /// positions; on X-Trans the two other colors in RGB order, and
    /// `None` last.
    pub others: [Option<Neighbors>; 3],
}

impl Candidate {
    /// Whether the other colors' adjacent sites stand out from their
    /// own background the way this site does (brighter for a hot one,
    /// darker for a dead one) by both `factor` and `sigmas` of the
    /// noise: a picture, then, not a defect.
    ///
    /// `factor` below one is taken as one.
    pub fn others_follow(&self, factor: f32, sigmas: f32) -> bool {
        self.others_factor(sigmas) > factor.max(1.0)
    }

    /// How far the other colors follow this site: over the positions
    /// whose adjacent site stands more than `sigmas` of the noise
    /// beyond its background the way this site does, the largest
    /// factor it stands by (infinite over a background at zero), and
    /// one when no position does. The repair spares the site when this
    /// is above its others factor.
    pub fn others_factor(&self, sigmas: f32) -> f32 {
        self.others
            .iter()
            .flatten()
            .filter_map(|n| {
                if self.hot {
                    (n.near - n.background > sigmas * n.sigma).then(|| n.near / n.background)
                } else {
                    (n.background - n.near > sigmas * n.sigma).then(|| n.background / n.near)
                }
            })
            .fold(1.0, f32::max)
    }
}

/// One class of the sites round a candidate that the others test
/// reads: offsets of its adjacent sites and of its sites two and three
/// out, and their color.
struct Class {
    near: Vec<isize>,
    ring: Vec<isize>,
    color: usize,
}

/// What the tests read round a site of one position of the pattern:
/// its color, the offsets of its same-color neighbors, and the other
/// classes. The same-color offsets are a fixed `N` (eight on Bayer,
/// sixteen on X-Trans), a site with fewer padded with its first one
/// again, which the largest and the smallest of them do not see: every
/// site reads in a loop of one length, which the compiler unrolls.
struct Site<const N: usize> {
    color: usize,
    same: [isize; N],
    others: Vec<Class>,
}

/// The most same-color sites any X-Trans site reads: a green's in its
/// 5x5 (a red's or blue's 7x7 holds eleven).
const XTRANS_SAME: usize = 16;
/// Bayer's: the eight two photosites away.
const BAYER_SAME: usize = 8;

/// Offsets into a buffer `width` wide.
fn linear(offsets: impl IntoIterator<Item = (isize, isize)>, width: usize) -> Vec<isize> {
    offsets
        .into_iter()
        .map(|(dy, dx)| dy * width as isize + dx)
        .collect()
}

/// The tests' reads for each site of `pattern`, 2x2 Bayer or X-Trans.
///
/// Bayer: the same-color neighbors two photosites away (eight), and the
/// other three positions of the 2x2, one photosite across, down and
/// diagonal, each its sites in the 3x3 and in the 7x7 outside it.
/// X-Trans: the same color's sites in the 5x5 for green and the 7x7 for
/// red and blue, and the other two colors, each its sites in the 3x3
/// and in the 7x7 outside it.
fn layout<const N: usize>(pattern: &CfaPattern, width: usize) -> Phases<Site<N>> {
    let xtrans = pattern.is_xtrans();
    let padded = |same: Vec<isize>| -> [isize; N] {
        assert!(
            !same.is_empty() && same.len() <= N,
            "{} same-color sites",
            same.len()
        );
        std::array::from_fn(|k| same.get(k).copied().unwrap_or(same[0]))
    };
    let color_at = |y: usize, x: usize| {
        pattern
            .color_at(y, x)
            .rgb_index()
            .expect("repaired patterns are RGB")
    };
    let adjacent = |&(dy, dx): &(isize, isize)| dy.abs() <= 1 && dx.abs() <= 1;
    Phases::build(pattern, |y, x| {
        let color = color_at(y, x);
        if xtrans {
            let others = (0..3)
                .filter(|&c| c != color)
                .map(|c| {
                    let (near, ring): (Vec<_>, Vec<_>) = of_color(pattern, y, x, c, 3)
                        .into_iter()
                        .partition(adjacent);
                    Class {
                        near: linear(near, width),
                        ring: linear(ring, width),
                        color: c,
                    }
                })
                .collect();
            Site {
                color,
                // Green its 5x5, red and blue their 7x7: see the
                // module doc.
                same: padded(linear(
                    of_color(pattern, y, x, color, if color == 1 { 2 } else { 3 }),
                    width,
                )),
                others,
            }
        } else {
            let same = [-2isize, 0, 2]
                .into_iter()
                .flat_map(|dy| [-2isize, 0, 2].map(|dx| (dy, dx)))
                .filter(|&d| d != (0, 0));
            let others = [(0isize, 1isize), (1, 0), (1, 1)]
                .into_iter()
                .map(|(py, px)| {
                    let (near, ring): (Vec<_>, Vec<_>) = (-3isize..=3)
                        .filter(|d| (d - py).rem_euclid(2) == 0)
                        .flat_map(|dy| {
                            (-3isize..=3)
                                .filter(|d| (d - px).rem_euclid(2) == 0)
                                .map(move |dx| (dy, dx))
                        })
                        .partition(adjacent);
                    Class {
                        near: linear(near, width),
                        ring: linear(ring, width),
                        color: color_at(y + py as usize, x + px as usize),
                    }
                })
                .collect();
            Site {
                color,
                same: padded(linear(same, width)),
                others,
            }
        }
    })
}

/// The photosites the same-color tests flag, with the other colors
/// round each measured; the repair keeps those whose other colors do
/// not follow. Sites within three of the edge are never candidates.
pub fn candidates(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    model: &NoiseModel,
    sigmas: f32,
    ratio: f32,
) -> Result<Vec<Candidate>> {
    if !repairs(pattern) {
        return Err(Error::Unsupported(format!(
            "hot pixel repair needs a 2x2 Bayer or an X-Trans pattern, got {pattern}"
        )));
    }
    if samples.len() != width * height {
        return Err(Error::Unsupported(
            "hot pixels: sample buffer size mismatch".into(),
        ));
    }
    if width < 7 || height < 7 || sigmas <= 0.0 {
        return Ok(Vec::new());
    }
    let ratio = ratio.max(1.0);
    let size = (width, height);
    Ok(if pattern.is_xtrans() {
        scan(
            samples,
            size,
            &layout::<XTRANS_SAME>(pattern, width),
            model,
            sigmas,
            ratio,
        )
    } else {
        scan(
            samples,
            size,
            &layout::<BAYER_SAME>(pattern, width),
            model,
            sigmas,
            ratio,
        )
    })
}

/// [`candidates`] over a layout: the same-color tests at every site
/// three or more from the edge, then the others measured round each
/// site they flag.
fn scan<const N: usize>(
    samples: &[f32],
    (width, height): (usize, usize),
    layout: &Phases<Site<N>>,
    model: &NoiseModel,
    k: f32,
    ratio: f32,
) -> Vec<Candidate> {
    // The model's noise, never below a sensor's read noise: where the
    // samples are clamped at black the estimate sees no spread and
    // can put the read noise at zero, and then a site a few units
    // above black over neighbors at zero is many sigmas out.
    let sigma = |c: usize, level: f32| model.sigma(c, level.max(0.0)).max(MIN_SIGMA);
    let at = |i: usize, o: isize| samples[(i as isize + o) as usize];
    (3..height - 3)
        .into_par_iter()
        .flat_map_iter(|y| {
            let mut row = Vec::new();
            let sites = layout.row(y);
            let sites = sites.iter().cycle().skip(3 % sites.len());
            for (x, site) in (3..width - 3).zip(sites) {
                let i = y * width + x;
                let v = samples[i];
                let near = &samples[i - 3 * width - 3..=i + 3 * width + 3];
                let base = (3 * width + 3) as isize;
                let mut mx = f32::NEG_INFINITY;
                let mut mn = f32::INFINITY;
                for &o in &site.same {
                    let n = near[(base + o) as usize];
                    mx = mx.max(n);
                    mn = mn.min(n);
                }
                let c = site.color;
                let hot = if v - mx > k * sigma(c, mx) && v > mx * ratio {
                    true
                } else if mn - v > k * sigma(c, mn) && v * ratio < mn {
                    false
                } else {
                    continue;
                };
                // The other classes round the site, each judged on its
                // adjacent sites against its own sites two and three
                // out.
                let mut others = [None; 3];
                for (slot, class) in others.iter_mut().zip(&site.others) {
                    let mut near = if hot {
                        f32::NEG_INFINITY
                    } else {
                        f32::INFINITY
                    };
                    for &o in &class.near {
                        let s = at(i, o);
                        near = if hot { near.max(s) } else { near.min(s) };
                    }
                    let mut ring = [0f32; 40];
                    let n = class.ring.len();
                    for (r, &o) in ring.iter_mut().zip(&class.ring) {
                        *r = at(i, o);
                    }
                    let ring = &mut ring[..n];
                    ring.sort_unstable_by(f32::total_cmp);
                    let background = ring[n / 2];
                    *slot = Some(Neighbors {
                        near,
                        background,
                        sigma: sigma(class.color, background),
                    });
                }
                row.push(Candidate {
                    index: i,
                    hot,
                    value: v,
                    replacement: if hot { mx } else { mn },
                    others,
                });
            }
            row
        })
        .collect()
}

/// Repair `samples` (one per photosite, levels normalized, no gains
/// applied) in place. `model` is the noise of the samples as they are.
pub fn fix_hot_pixels(
    samples: &mut [f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    model: &NoiseModel,
    options: &HotPixelOptions,
) -> Result<HotPixelStats> {
    // Decisions on the values as shot, then the replacements.
    let found = candidates(
        samples,
        width,
        height,
        pattern,
        model,
        options.sigmas,
        options.ratio,
    )?;
    let mut stats = HotPixelStats::default();
    for c in found {
        if c.others_follow(options.others, OTHERS_SIGMAS) {
            continue;
        }
        samples[c.index] = c.replacement;
        if c.hot {
            stats.hot += 1;
        } else {
            stats.dead += 1;
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::noise::tests::Gauss;

    fn field(w: usize, h: usize, level: f32, model: &NoiseModel, seed: u64) -> Vec<f32> {
        let mut g = Gauss(seed);
        (0..w * h)
            .map(|i| level + g.next() * model.sigma(i % 2, level))
            .collect()
    }

    #[test]
    fn finds_the_outliers_and_nothing_else() {
        let (w, h) = (128, 96);
        let model = NoiseModel {
            a: [2e-3; 3],
            b: [1e-5; 3],
        };
        let mut samples = field(w, h, 0.4, &model, 5);
        let sigma = model.sigma(1, 0.4);
        let hot = [(10, 11), (50, 77), (92, 3), (40, 40)];
        let dead = [(20, 64), (70, 100)];
        // Defects: several times the level, or a small fraction of it,
        // which is many sigmas either way.
        for &(y, x) in &hot {
            samples[y * w + x] = 2.0;
        }
        for &(y, x) in &dead {
            samples[y * w + x] = 0.02;
        }
        let before = samples.clone();
        let stats = fix_hot_pixels(
            &mut samples,
            w,
            h,
            &CfaPattern::rggb(),
            &model,
            &HotPixelOptions::default(),
        )
        .unwrap();
        assert_eq!(stats, HotPixelStats { hot: 4, dead: 2 });
        for &(y, x) in hot.iter().chain(&dead) {
            let v = samples[y * w + x];
            assert!((v - 0.4).abs() < 4.0 * sigma, "({x},{y}) -> {v}");
        }
        let changed = samples.iter().zip(&before).filter(|(a, b)| a != b).count();
        assert_eq!(changed, 6, "nothing else touched");
    }

    #[test]
    fn a_small_highlight_survives_and_a_dot_does_not() {
        let (w, h) = (64, 64);
        let model = NoiseModel {
            a: [1e-3; 3],
            b: [1e-5; 3],
        };
        let mut samples = field(w, h, 0.1, &model, 9);
        // A 5x5 highlight: every photosite in it has a same-color
        // neighbor two away that is also in it.
        for y in 20..25 {
            for x in 20..25 {
                samples[y * w + x] = 0.9;
            }
        }
        // A single bright photosite.
        samples[40 * w + 40] = 0.9;
        let stats = fix_hot_pixels(
            &mut samples,
            w,
            h,
            &CfaPattern::rggb(),
            &model,
            &HotPixelOptions::default(),
        )
        .unwrap();
        assert_eq!(stats, HotPixelStats { hot: 1, dead: 0 });
        assert!(samples[40 * w + 40] < 0.2);
        for y in 20..25 {
            for x in 20..25 {
                assert_eq!(samples[y * w + x], 0.9, "({x},{y})");
            }
        }
    }

    /// The base-ISO noise of a clean sensor, and a flat grey mosaic
    /// carrying it.
    fn base_iso() -> NoiseModel {
        NoiseModel {
            a: [1e-3; 3],
            b: [1e-5; 3],
        }
    }

    fn repair(samples: &mut [f32], w: usize, h: usize, options: HotPixelOptions) -> HotPixelStats {
        fix_hot_pixels(samples, w, h, &CfaPattern::rggb(), &base_iso(), &options).unwrap()
    }

    const OLD_RULE: HotPixelOptions = HotPixelOptions {
        sigmas: DEFAULT_SIGMAS,
        ratio: DEFAULT_RATIO,
        others: f32::INFINITY,
    };

    #[test]
    fn a_one_color_site_goes_and_a_white_glint_stays() {
        let (w, h) = (96, 64);
        let model = base_iso();
        let clean = field(w, h, 0.1, &model, 21);
        let mut samples = clean.clone();
        // A hot blue photosite (RGGB: odd row, odd column), the defect
        // the backlit couple has: one color far up, the sites round it
        // at the background.
        let defect = 21 * w + 21;
        samples[defect] = 0.9;
        // A glint two photosites across: one 2x2 cell, every color lit.
        let glint = [(20, 60), (20, 61), (21, 60), (21, 61)];
        // A catchlight on a site: a 3x3 white point, the center
        // brightest, as the lens spreads one.
        let catchlight: Vec<(usize, usize, f32)> = (-1isize..=1)
            .flat_map(|dy| (-1isize..=1).map(move |dx| (dy, dx)))
            .map(|(dy, dx)| {
                let v = if dy == 0 && dx == 0 { 0.95 } else { 0.5 };
                ((44 + dy) as usize, (40 + dx) as usize, v)
            })
            .collect();
        for &(y, x) in &glint {
            samples[y * w + x] = 0.9;
        }
        for &(y, x, v) in &catchlight {
            samples[y * w + x] = v;
        }
        let shot = samples.clone();

        // The same-color test alone takes the glint's four sites and the
        // catchlight's center along with the defect.
        let mut old = shot.clone();
        let stats = repair(&mut old, w, h, OLD_RULE);
        assert_eq!(stats.hot, 6, "{stats:?}");
        assert!(
            old[44 * w + 40] < 0.6,
            "the old rule darkened the catchlight"
        );

        let stats = repair(&mut samples, w, h, HotPixelOptions::default());
        assert_eq!(stats, HotPixelStats { hot: 1, dead: 0 });
        assert!((samples[defect] - 0.1).abs() < 0.02, "{}", samples[defect]);
        let changed: Vec<usize> = (0..w * h).filter(|&i| samples[i] != shot[i]).collect();
        assert_eq!(changed, [defect], "glint and catchlight as shot");
    }

    #[test]
    fn a_dead_site_goes_and_a_dark_speck_stays() {
        let (w, h) = (64, 64);
        let model = base_iso();
        let mut samples = field(w, h, 0.4, &model, 33);
        // A dead green site, and a speck of dark texture one cell
        // across (every color down).
        let defect = 30 * w + 31;
        samples[defect] = 0.01;
        for (y, x) in [(10, 50), (10, 51), (11, 50), (11, 51)] {
            samples[y * w + x] = 0.02;
        }
        let shot = samples.clone();
        let stats = repair(&mut samples, w, h, HotPixelOptions::default());
        assert_eq!(stats, HotPixelStats { hot: 0, dead: 1 });
        let changed: Vec<usize> = (0..w * h).filter(|&i| samples[i] != shot[i]).collect();
        assert_eq!(changed, [defect]);
        let mut old = shot;
        assert_eq!(repair(&mut old, w, h, OLD_RULE).dead, 5);
    }

    #[test]
    fn fine_texture_is_left_alone() {
        // Texture at the scale of one 2x2 cell, with a contrast wide
        // enough that cells often stand three times above every cell
        // round them: specks of foliage or fur at base ISO, where the
        // noise is no help. Every color of a cell moves together, as
        // anything the lens drew does, with a color of its own.
        let (w, h) = (256, 256);
        let model = base_iso();
        let mut g = Gauss(77);
        let cells: Vec<f32> = (0..(w / 2) * (h / 2))
            .map(|_| 0.05 * (1.2 * g.next()).exp())
            .collect();
        let tint = [0.6f32, 1.0, 0.8];
        let mut samples: Vec<f32> = (0..w * h)
            .map(|i| {
                let (y, x) = (i / w, i % w);
                let c = CfaPattern::rggb().color_at(y, x).rgb_index().unwrap();
                let v = (cells[(y / 2) * (w / 2) + x / 2] * tint[c]).min(1.0);
                v + g.next() * model.sigma(c, v)
            })
            .collect();
        let shot = samples.clone();
        let old = repair(&mut shot.clone(), w, h, OLD_RULE);
        assert!(
            old.hot + old.dead > 20,
            "the old rule takes texture: {old:?}"
        );
        let stats = repair(&mut samples, w, h, HotPixelOptions::default());
        assert_eq!(stats, HotPixelStats::default());
        assert_eq!(samples, shot);
    }

    #[test]
    fn the_others_are_judged_on_their_own_background() {
        // A hot red site on a background the scene makes uneven across
        // colors (strong green, faint blue): the other colors are
        // judged against their own level, not the candidate's.
        let (w, h) = (48, 48);
        let model = base_iso();
        let mut g = Gauss(5);
        let level = [0.05f32, 0.4, 0.02];
        let mut samples: Vec<f32> = (0..w * h)
            .map(|i| {
                let c = CfaPattern::rggb()
                    .color_at(i / w, i % w)
                    .rgb_index()
                    .unwrap();
                level[c] + g.next() * model.sigma(c, level[c])
            })
            .collect();
        samples[20 * w + 20] = 0.6;
        let found = candidates(
            &samples,
            w,
            h,
            &CfaPattern::rggb(),
            &model,
            DEFAULT_SIGMAS,
            DEFAULT_RATIO,
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        let c = found[0];
        assert!(c.hot && c.index == 20 * w + 20);
        for (n, want) in c.others.iter().flatten().zip([0.4, 0.4, 0.02]) {
            assert!((n.background - want).abs() < 0.2 * want, "{n:?}");
        }
        assert!(!c.others_follow(DEFAULT_OTHERS, OTHERS_SIGMAS));
        assert_eq!(
            repair(&mut samples, w, h, HotPixelOptions::default()).hot,
            1
        );
    }

    /// A flat field of three colors at their own levels, on `pattern`.
    fn colored(w: usize, h: usize, pattern: &CfaPattern, seed: u64) -> Vec<f32> {
        let model = base_iso();
        let level = [0.10f32, 0.20, 0.08];
        let mut g = Gauss(seed);
        (0..w * h)
            .map(|i| {
                let c = pattern.color_at(i / w, i % w).rgb_index().unwrap();
                level[c] + g.next() * model.sigma(c, level[c])
            })
            .collect()
    }

    fn fix_on(samples: &mut [f32], w: usize, h: usize, pattern: &CfaPattern) -> HotPixelStats {
        fix_hot_pixels(
            samples,
            w,
            h,
            pattern,
            &base_iso(),
            &HotPixelOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn every_phase_and_parity_reads_its_neighbors_right() {
        let (w, h) = (40usize, 40usize);
        for phase in 0..4 {
            let pattern = CfaPattern::rggb().shifted(phase & 1, phase >> 1);
            for (cy, cx) in [(20usize, 20usize), (20, 21), (21, 20), (21, 21)] {
                let ci = cy * w + cx;
                let seed = 7 + (phase * 1000 + ci) as u64;
                let at = |dy: isize, dx: isize| {
                    (cy as isize + dy) as usize * w + (cx as isize + dx) as usize
                };
                let tag = format!("phase {phase}, site ({cy},{cx})");

                // A lone hot site goes, and nothing else changes.
                let mut s = colored(w, h, &pattern, seed);
                s[ci] = 0.9;
                let shot = s.clone();
                assert_eq!(fix_on(&mut s, w, h, &pattern).hot, 1, "{tag}");
                let changed: Vec<usize> = (0..w * h).filter(|&i| s[i] != shot[i]).collect();
                assert_eq!(changed, [ci], "{tag}");

                // Any one adjacent site lit to two and a half times its
                // level spares it (short of three, where the lit site
                // would be a candidate of its own).
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        if (dy, dx) == (0, 0) {
                            continue;
                        }
                        let mut s = shot.clone();
                        s[at(dy, dx)] *= 2.5;
                        assert_eq!(
                            fix_on(&mut s, w, h, &pattern).hot,
                            0,
                            "{tag}: adjacent ({dy},{dx}) lit"
                        );
                    }
                }

                // A site two or three out lit does not.
                for (dy, dx) in [
                    (0isize, 3isize),
                    (2, 1),
                    (-2, -3),
                    (3, 0),
                    (1, -2),
                    (3, 3),
                    (-3, 1),
                    (-1, -3),
                ] {
                    let mut s = shot.clone();
                    s[at(dy, dx)] *= 2.5;
                    assert_eq!(
                        fix_on(&mut s, w, h, &pattern).hot,
                        1,
                        "{tag}: ring ({dy},{dx}) lit"
                    );
                }

                // The dead mirror: a lone dark site goes, and one with
                // any adjacent site dark too stays.
                let mut dark = colored(w, h, &pattern, seed + 1);
                dark[ci] = 0.001;
                let mut s = dark.clone();
                assert_eq!(fix_on(&mut s, w, h, &pattern).dead, 1, "{tag}: dead");
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        if (dy, dx) == (0, 0) {
                            continue;
                        }
                        let mut s = dark.clone();
                        s[at(dy, dx)] = 0.001;
                        assert_eq!(
                            fix_on(&mut s, w, h, &pattern).dead,
                            0,
                            "{tag}: dead, adjacent ({dy},{dx}) dark"
                        );
                    }
                }
            }

            // The edge band: three sites, never judged.
            for (x, want) in [(2usize, 0usize), (3, 1), (w - 3, 0), (w - 4, 1)] {
                let mut s = colored(w, h, &pattern, 99);
                s[20 * w + x] = 0.9;
                assert_eq!(
                    fix_on(&mut s, w, h, &pattern).hot,
                    want,
                    "phase {phase}, edge x = {x}"
                );
            }
        }
    }

    #[test]
    fn read_noise_at_clamped_black_is_not_a_defect() {
        // A dark frame's shadows: read noise of about three units of a
        // 14-bit raw, clamped at black as the levels are, and the
        // noise estimate's usual answer there, a read noise of zero.
        let (w, h) = (512, 512);
        let read = 1.8e-4f32;
        let mut g = Gauss(41);
        let mut samples: Vec<f32> = (0..w * h).map(|_| (g.next() * read).max(0.0)).collect();
        let blind = NoiseModel {
            a: [1e-4; 3],
            b: [0.0; 3],
        };
        let shot = samples.clone();
        let stats = fix_hot_pixels(
            &mut samples,
            w,
            h,
            &CfaPattern::rggb(),
            &blind,
            &HotPixelOptions::default(),
        )
        .unwrap();
        assert_eq!(stats, HotPixelStats::default());
        assert_eq!(samples, shot);
        // A real defect in the same dark still goes: forty units.
        samples[100 * w + 101] = 0.0025;
        let stats = fix_hot_pixels(
            &mut samples,
            w,
            h,
            &CfaPattern::rggb(),
            &blind,
            &HotPixelOptions::default(),
        )
        .unwrap();
        assert_eq!(stats, HotPixelStats { hot: 1, dead: 0 });
    }

    /// X-Trans at several phases, every site of the period: a lone hot
    /// or dead site goes and nothing else changes; any adjacent site lit
    /// to two and a half times its level (or dark) with it spares it, a
    /// same-color one of a 2x2 green block as much as one of another
    /// color; a site of another color two or three out lit to twice its
    /// level does not. Twice there, not the Bayer test's two and a
    /// half: the ring was set when a red or blue site read the five
    /// same-color sites of its 5x5, where a noisy site lit by two and
    /// a half could stand three times above all five, a candidate of
    /// its own; the 7x7's eleven leave it at two, which the texture
    /// test holds.
    #[test]
    fn every_x_trans_site_reads_its_neighbors_right() {
        let (w, h) = (40usize, 40usize);
        for (px, py) in [(0usize, 0usize), (2, 2), (5, 3)] {
            let pattern = CfaPattern::xtrans().shifted(px, py);
            let color = |y: isize, x: isize| pattern.color_at(y as usize, x as usize);
            for (cy, cx) in (17..23usize).flat_map(|y| (17..23usize).map(move |x| (y, x))) {
                let ci = cy * w + cx;
                let seed = 5 + (px * 1000 + ci) as u64;
                let at = |dy: isize, dx: isize| {
                    (cy as isize + dy) as usize * w + (cx as isize + dx) as usize
                };
                let tag = format!("phase ({px},{py}), site ({cy},{cx})");

                let mut s = colored(w, h, &pattern, seed);
                s[ci] = 0.9;
                let shot = s.clone();
                assert_eq!(fix_on(&mut s, w, h, &pattern).hot, 1, "{tag}");
                let changed: Vec<usize> = (0..w * h).filter(|&i| s[i] != shot[i]).collect();
                assert_eq!(changed, [ci], "{tag}");

                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        if (dy, dx) == (0, 0) {
                            continue;
                        }
                        let mut s = shot.clone();
                        s[at(dy, dx)] *= 2.5;
                        assert_eq!(
                            fix_on(&mut s, w, h, &pattern).hot,
                            0,
                            "{tag}: adjacent ({dy},{dx}) lit"
                        );
                    }
                }

                let own = color(cy as isize, cx as isize);
                for dy in -3isize..=3 {
                    for dx in -3isize..=3 {
                        let (y, x) = (cy as isize + dy, cx as isize + dx);
                        if dy.abs().max(dx.abs()) < 2 || color(y, x) == own {
                            continue;
                        }
                        let mut s = shot.clone();
                        s[at(dy, dx)] *= 2.0;
                        assert_eq!(
                            fix_on(&mut s, w, h, &pattern).hot,
                            1,
                            "{tag}: ring ({dy},{dx}) lit"
                        );
                    }
                }

                let mut dark = colored(w, h, &pattern, seed + 1);
                dark[ci] = 0.001;
                let mut s = dark.clone();
                assert_eq!(fix_on(&mut s, w, h, &pattern).dead, 1, "{tag}: dead");
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        if (dy, dx) == (0, 0) {
                            continue;
                        }
                        let mut s = dark.clone();
                        s[at(dy, dx)] = 0.001;
                        assert_eq!(
                            fix_on(&mut s, w, h, &pattern).dead,
                            0,
                            "{tag}: dead, adjacent ({dy},{dx}) dark"
                        );
                    }
                }
            }
        }
    }

    /// Texture at the scale of a 2x2 cell with a wide contrast, every
    /// color of a cell moving together with a tint of its own, at base
    /// ISO: what `fine_texture_is_left_alone` makes, on any pattern.
    fn cell_texture(w: usize, h: usize, pattern: &CfaPattern, seed: u64) -> Vec<f32> {
        let model = base_iso();
        let mut g = Gauss(seed);
        let cells: Vec<f32> = (0..(w / 2) * (h / 2))
            .map(|_| 0.05 * (1.2 * g.next()).exp())
            .collect();
        let tint = [0.6f32, 1.0, 0.8];
        (0..w * h)
            .map(|i| {
                let (y, x) = (i / w, i % w);
                let c = pattern.color_at(y, x).rgb_index().unwrap();
                let v = (cells[(y / 2) * (w / 2) + x / 2] * tint[c]).min(1.0);
                v + g.next() * model.sigma(c, v)
            })
            .collect()
    }

    /// On X-Trans, as on Bayer: a one-color site goes, a glint that
    /// lights a 2x2 of every color and a catchlight spread over a 3x3
    /// stay.
    #[test]
    fn x_trans_defects_go_and_pictures_stay() {
        let (w, h) = (96, 96);
        let pattern = CfaPattern::xtrans().shifted(1, 4);
        let mut samples = colored(w, h, &pattern, 61);
        let defects = [21 * w + 21, 60 * w + 33, 70 * w + 80];
        for &i in &defects {
            samples[i] = 0.9;
        }
        for (y, x) in [(20, 60), (20, 61), (21, 60), (21, 61)] {
            samples[y * w + x] = 0.9;
        }
        for dy in 0..3 {
            for dx in 0..3 {
                samples[(44 + dy) * w + 40 + dx] = if (dy, dx) == (1, 1) { 0.95 } else { 0.5 };
            }
        }
        let shot = samples.clone();
        let stats = fix_on(&mut samples, w, h, &pattern);
        assert_eq!(stats, HotPixelStats { hot: 3, dead: 0 });
        let changed: Vec<usize> = (0..w * h).filter(|&i| samples[i] != shot[i]).collect();
        assert_eq!(changed, defects);
    }

    /// Texture at the scale of a 2x2 cell on X-Trans, over seeds and
    /// phases, against the same texture on Bayer: the same-color test
    /// alone takes hundreds of sites a frame, and the repair takes
    /// no more than it does on Bayer.
    ///
    /// With the red and blue sites' same-color test over their 5x5
    /// (five neighbors, to Bayer's eight) a speck of texture cleared
    /// them all often enough that over 300 frames (100 seeds, three
    /// phases) the repair took 1.95 sites a frame, against Bayer's
    /// 0.14, and in half the frames more than one. Over the 7x7 (eleven
    /// neighbors) it is 0.02.
    #[test]
    fn x_trans_texture_is_left_alone_as_on_bayer() {
        let (w, h) = (252, 252);
        let model = base_iso();
        let count = |patterns: &[CfaPattern]| {
            let mut total = 0;
            for p in patterns {
                // Seed 0 is a dead frame: the xorshift stays at zero.
                for seed in 1..=10u64 {
                    let mut s = cell_texture(w, h, p, seed);
                    let shot = s.clone();
                    let stats = fix_on(&mut s, w, h, p);
                    let changed = (0..w * h).filter(|&i| s[i] != shot[i]).count();
                    assert_eq!(changed, stats.hot + stats.dead);
                    total += changed;
                }
            }
            total
        };
        let xtrans = [
            CfaPattern::xtrans(),
            CfaPattern::xtrans().shifted(1, 4),
            CfaPattern::xtrans().shifted(3, 2),
        ];
        let bayer = [
            CfaPattern::rggb(),
            CfaPattern::rggb().shifted(1, 0),
            CfaPattern::rggb().shifted(1, 1),
        ];
        let shot = cell_texture(w, h, &xtrans[1], 78);
        let old = fix_hot_pixels(&mut shot.clone(), w, h, &xtrans[1], &model, &OLD_RULE).unwrap();
        assert!(
            old.hot + old.dead > 200,
            "the old rule takes texture: {old:?}"
        );
        let (on_xtrans, on_bayer) = (count(&xtrans), count(&bayer));
        assert!(
            on_xtrans <= on_bayer.max(1),
            "{on_xtrans} against Bayer's {on_bayer}"
        );
    }

    #[test]
    fn other_patterns_are_refused() {
        let all_green = CfaPattern::new(6, 6, vec![crate::raw::CfaColor::Green; 36]).unwrap();
        assert!(!repairs(&all_green));
        assert!(
            fix_hot_pixels(
                &mut [0.0; 64],
                8,
                8,
                &all_green,
                &base_iso(),
                &HotPixelOptions::default()
            )
            .is_err()
        );
    }
}
