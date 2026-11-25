//! Focus breathing against the truth: a bed of frames whose
//! magnification runs smoothly through the stack, as a lens's does when
//! its focus is turned, and the chain's placings measured against the
//! transforms the frames were made by.

use rayon::prelude::*;

use super::{Options, fit_chain, luminance_of};
use crate::register::Refine;
use crate::register::Transform;

/// A lattice value at `(ix, iy)` in 0..1, from the coordinates and a
/// seed alone.
fn lattice(ix: i64, iy: i64, seed: u64) -> f64 {
    let mut h = (ix as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(seed.wrapping_mul(0x1656_67B1_9E37_79F9));
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Value noise on a lattice of `cell` pixels, smoothstepped.
fn octave(x: f64, y: f64, cell: f64) -> f64 {
    let (gx, gy) = (x / cell, y / cell);
    let (i, j) = (gx.floor(), gy.floor());
    let (fx, fy) = (gx - i, gy - j);
    let ease = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (ease(fx), ease(fy));
    let (i, j) = (i as i64, j as i64);
    let a = lattice(i, j, 0) + sx * (lattice(i + 1, j, 0) - lattice(i, j, 0));
    let b = lattice(i, j + 1, 0) + sx * (lattice(i + 1, j + 1, 0) - lattice(i, j + 1, 0));
    a + sy * (b - a)
}

const CELLS: [f64; 5] = [160.0, 80.0, 40.0, 20.0, 10.0];

/// The scene at a point, defocused by `sigma`: each octave of the
/// texture multiplied by a Gaussian's response at its period, and a
/// sprinkling of small bright points whose out-of-focus disc grows
/// with the blur, its energy kept, as a highlight's bokeh does.
fn scene(x: f64, y: f64, sigma: f64) -> f32 {
    let mut v = 0.6;
    let mut amplitude = 0.35;
    for cell in CELLS {
        let response = (-2.0 * std::f64::consts::PI.powi(2) * sigma * sigma / (cell * cell)).exp();
        v += amplitude * response * (octave(x, y, cell) - 0.5);
        amplitude *= 0.85;
    }
    // One point in some cells of a 48-pixel grid, at a place of its own.
    const GRID: f64 = 48.0;
    let (ci, cj) = ((x / GRID).floor() as i64, (y / GRID).floor() as i64);
    for dj in -1..=1 {
        for di in -1..=1 {
            let (i, j) = (ci + di, cj + dj);
            if lattice(i, j, 7) > 0.35 {
                continue;
            }
            let px = (i as f64 + 0.2 + 0.6 * lattice(i, j, 8)) * GRID;
            let py = (j as f64 + 0.2 + 0.6 * lattice(i, j, 9)) * GRID;
            let r = (x - px).hypot(y - py);
            // A disc of radius 1.5 in focus, widened by the blur: the
            // light it holds spread over its area.
            let radius = 1.5f64.hypot(2.0 * sigma);
            let edge = ((radius - r) + 0.5).clamp(0.0, 1.0);
            v += 1.5 * edge * (1.5 / radius).powi(2);
        }
    }
    v as f32
}

/// What a bed is made of.
#[derive(Debug, Clone, Copy)]
pub(super) struct Bed {
    pub width: usize,
    pub height: usize,
    pub frames: usize,
    /// The magnification's change per frame, as a fraction: 0.0015 is
    /// a frame 0.15% larger than the one before.
    pub breathing: f64,
    /// The change in that change per frame, so the curve need not be
    /// straight.
    pub bend: f64,
    /// Pixels of shift, and degrees of turn, a hand puts between
    /// frames at most: each frame's own, not a drift.
    pub shake: f64,
    pub turn: f64,
    /// White noise's deviation, in the scene's units (it sits near
    /// 0.6).
    pub noise: f64,
    /// How fast the defocus grows away from a frame's sharp band:
    /// sigma per tenth of the width. 1 is the stack the module's own
    /// tests build; 4 leaves most of each frame a smear.
    pub steep: f64,
    /// A frame out of focus everywhere: its index, and the defocus
    /// added to every pixel of it.
    pub smeared: Option<(usize, f64)>,
}

impl Bed {
    pub fn new(width: usize, height: usize, frames: usize) -> Self {
        Self {
            width,
            height,
            frames,
            breathing: 0.0015,
            bend: 0.0,
            shake: 0.0,
            turn: 0.0,
            noise: 0.0,
            steep: 1.0,
            smeared: None,
        }
    }

    fn middle(&self) -> [f64; 2] {
        [
            (self.width - 1) as f64 / 2.0,
            (self.height - 1) as f64 / 2.0,
        ]
    }

    /// Frame `k`'s pixel to the scene.
    fn scene_of(&self, k: usize) -> Transform {
        let step = k as f64 - (self.frames / 2) as f64;
        let scale = 1.0 + self.breathing * step + self.bend * step * step;
        let hand = |seed: u64| 2.0 * lattice(k as i64, 0, seed) - 1.0;
        Transform::similarity(
            scale,
            (self.turn * hand(11)).to_radians(),
            self.middle(),
            [self.shake * hand(12), self.shake * hand(13)],
        )
    }

    /// Frame `k`'s true placing onto the middle frame, as the fit
    /// reports it: the middle frame's coordinates to frame `k`'s.
    pub fn truth(&self, k: usize) -> Transform {
        let r = self.frames / 2;
        self.scene_of(k)
            .inverse()
            .expect("a similarity inverts")
            .compose(&self.scene_of(r))
    }

    /// Frame `k`, RGB: sharp in its own band of the scene's x, blurred
    /// either side of it.
    pub fn frame(&self, k: usize) -> Vec<f32> {
        let to_scene = self.scene_of(k);
        let focus = (k as f64 + 0.5) / self.frames as f64 * self.width as f64;
        let ramp = self.width as f64 / 10.0 / self.steep;
        let width = self.width;
        let noise = self.noise;
        let smear = match self.smeared {
            Some((j, sigma)) if j == k => sigma,
            _ => 0.0,
        };
        (0..self.width * self.height)
            .into_par_iter()
            .flat_map_iter(|p| {
                let (x, y) = ((p % width) as f64, (p / width) as f64);
                let (u, v) = to_scene.apply(x, y);
                let mut s = scene(u, v, ((u - focus) / ramp).hypot(smear)) as f64;
                if noise > 0.0 {
                    // Roughly normal: the sum of four uniforms.
                    let n: f64 = (0..4)
                        .map(|i| lattice(p as i64, k as i64, 100 + i) - 0.5)
                        .sum::<f64>()
                        * 3f64.sqrt();
                    s += noise * n;
                }
                let s = s.max(1e-4) as f32;
                [s, s, s]
            })
            .collect()
    }
}

/// One frame's placing against its truth.
#[derive(Debug, Clone, Copy)]
pub(super) struct Placed {
    pub index: usize,
    pub scale: f64,
    pub true_scale: f64,
    /// The worst corner's distance from where the truth puts it.
    pub corner: f64,
    /// The middle's distance from where the truth puts it, and the
    /// turn's error in degrees.
    pub center: f64,
    pub turn: f64,
    pub used: bool,
}

/// A bed's frames as luminance planes, with each frame's true placing.
pub(super) struct Planes {
    pub width: usize,
    pub height: usize,
    pub lum: Vec<Vec<f32>>,
    pub truth: Vec<Transform>,
}

impl Planes {
    pub fn of_bed(bed: &Bed) -> Self {
        Self {
            width: bed.width,
            height: bed.height,
            lum: (0..bed.frames)
                .map(|k| luminance_of(&bed.frame(k), super::WORKING_LUMA))
                .collect(),
            truth: (0..bed.frames).map(|k| bed.truth(k)).collect(),
        }
    }

    pub fn of_shot(shot: &super::bed::Shot) -> Self {
        Self {
            width: shot.width,
            height: shot.height,
            lum: shot
                .frames()
                .iter()
                .map(|f| luminance_of(&f.data, super::WORKING_LUMA))
                .collect(),
            truth: (0..shot.frames)
                .map(|k| shot.view(k).inverse().expect("a similarity"))
                .collect(),
        }
    }
}

/// The chain over the planes, each frame against its truth.
pub(super) fn place(planes: &Planes, options: &Options) -> Vec<Placed> {
    let n = planes.lum.len();
    let r = n / 2;
    let chain = fit_chain::<crate::error::Error, ()>(
        n,
        planes.width,
        planes.height,
        r,
        &planes.lum[r],
        options,
        |i| Ok((planes.lum[i].clone(), ())),
        |_, _| {},
    )
    .expect("reads never fail here");
    let used: Vec<bool> = chain.dropped.iter().map(Option::is_none).collect();
    judge(planes, &chain.onto, &used)
}

fn judge(planes: &Planes, onto: &[Transform], used: &[bool]) -> Vec<Placed> {
    let (w, h) = (planes.width, planes.height);
    let (mx, my) = ((w - 1) as f64 / 2.0, (h - 1) as f64 / 2.0);
    (0..onto.len())
        .map(|k| {
            let truth = planes.truth[k];
            let got = onto[k];
            let (a, b) = got.apply(mx, my);
            let (c, d) = truth.apply(mx, my);
            Placed {
                index: k,
                scale: got.scale(),
                true_scale: truth.scale(),
                corner: got.corner_distance(&truth, w, h),
                center: (a - c).hypot(b - d),
                turn: (got.rotation() - truth.rotation()).to_degrees(),
                used: used[k],
            }
        })
        .collect()
}

/// The worst corner over the frames the chain kept.
pub(super) fn worst(placed: &[Placed]) -> f64 {
    placed
        .iter()
        .filter(|p| p.used)
        .map(|p| p.corner)
        .fold(0.0, f64::max)
}

fn show(name: &str, placed: &[Placed], verbose: bool) {
    println!("{name}: worst corner {:.3} px", worst(placed));
    if !verbose {
        return;
    }
    for p in placed {
        println!(
            "  {:>2} scale {:.6} true {:.6} ({:+.5}%)  corner {:.3} px  middle {:.3} px  turn {:+.4}{}",
            p.index,
            p.scale,
            p.true_scale,
            (p.scale / p.true_scale - 1.0) * 100.0,
            p.corner,
            p.center,
            p.turn,
            if p.used { "" } else { "  dropped" }
        );
    }
}

fn beds(n: usize) -> Vec<(String, Planes)> {
    let mut out = Vec::new();
    let only = std::env::var("BED_ONLY").ok();
    let mut add = |name: &str, planes: &dyn Fn() -> Planes| {
        if only.as_deref().is_none_or(|o| name.contains(o)) {
            out.push((format!("{n} frames, {name}"), planes()));
        }
    };
    let bed = Bed::new(480, 320, n);
    add("texture, breathing", &|| Planes::of_bed(&bed));
    add("texture, no breathing", &|| {
        Planes::of_bed(&Bed {
            breathing: 0.0,
            ..bed
        })
    });
    add("texture, breathing, noise, hand", &|| {
        Planes::of_bed(&Bed {
            noise: 0.01,
            shake: 3.0,
            turn: 0.1,
            ..bed
        })
    });
    add("texture, mostly blur", &|| {
        Planes::of_bed(&Bed { steep: 4.0, ..bed })
    });
    add("texture, one frame smeared", &|| {
        Planes::of_bed(&Bed {
            smeared: Some((n / 2 + 1, 4.0)),
            ..bed
        })
    });
    let shot = super::bed::Shot {
        frames: n,
        blur: if n > 20 { 40.0 } else { 16.0 },
        ..Default::default()
    };
    add("shot, breathing", &|| Planes::of_shot(&shot));
    add("shot, no breathing", &|| {
        Planes::of_shot(&super::bed::Shot {
            breathing: 0.0,
            ..shot
        })
    });
    out
}

/// The stack's options with the links refined as `refine` says.
fn refined(refine: Refine) -> Options {
    let mut options = Options::default();
    options.register.refine = refine;
    options
}

/// Every bed at 5, 11, and 31 frames (or `BED_FRAMES`), the gradient
/// fit alone against it refined by the phase: recovered against true
/// scale per frame with `BED_VERBOSE`, the worst corner always.
/// `cargo test -p greycard-core measure_breathing -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement: prints recovered against true scale over the beds"]
fn measure_breathing_recovery() {
    let verbose = std::env::var_os("BED_VERBOSE").is_some();
    let sizes: Vec<usize> = std::env::var("BED_FRAMES")
        .ok()
        .map(|s| s.split(',').filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_else(|| vec![5, 11, 31]);
    for &n in &sizes {
        for (name, planes) in beds(n) {
            for (how, refine) in [("gradient", Refine::None), ("phase", Refine::Phase)] {
                let t = std::time::Instant::now();
                let placed = place(&planes, &refined(refine));
                show(&format!("{name}: {how}"), &placed, verbose);
                if verbose {
                    println!("  ({:.1} s)", t.elapsed().as_secs_f64());
                }
            }
        }
    }
}

/// Real frames, as the placing pass reads them: a quick demosaic, the
/// mosaic's luminance.
fn real_planes(files: &[std::path::PathBuf]) -> (usize, usize, Vec<Vec<f32>>) {
    let mut out = Vec::new();
    let mut size = (0, 0);
    for f in files {
        let (frame, _) = crate::decode::decode_path_with_metadata(f).expect("a raw");
        let settings = crate::DevelopSettings {
            demosaic: crate::DemosaicMethod::Bilinear,
            highlights: crate::HighlightMode::Clip,
            chromatic_aberration: None,
            ..Default::default()
        };
        let d = crate::demosaic(&frame, &settings).expect("demosaiced");
        size = (d.image.width, d.image.height);
        out.push(crate::register::camera_luminance(&d.image));
    }
    (size.0, size.1, out)
}

/// One patch's shift: its middle, the shift, and the fit's residual.
type Patch = (f64, f64, f64, f64, f32);
/// Where the patches of `warped` sit against `template`'s, each fitted
/// by a shift alone, on a grid of `gx` by `gy` patches of `side`: what
/// a placing left at each place. Only patches whose fit is believed (a
/// residual under 0.3) and that the warp left inside the frame.
fn patch_field(
    template: &[f32],
    warped: &[f32],
    t: &Transform,
    (w, h): (usize, usize),
    (gx, gy): (usize, usize),
    side: usize,
) -> Vec<Patch> {
    let o = crate::register::Options {
        model: crate::register::Model::Translation,
        ..Default::default()
    };
    let mut out = Vec::new();
    for gj in 0..gy {
        for gi in 0..gx {
            let x0 = (gi * (w - side)) / (gx - 1);
            let y0 = (gj * (h - side)) / (gy - 1);
            let (u0, v0) = t.apply(x0 as f64, y0 as f64);
            let (u1, v1) = t.apply((x0 + side) as f64, (y0 + side) as f64);
            if u0 < 0.0 || v0 < 0.0 || u1 > w as f64 || v1 > h as f64 {
                continue;
            }
            let crop = |p: &[f32]| -> Vec<f32> {
                (0..side * side)
                    .map(|k| p[(y0 + k / side) * w + x0 + k % side])
                    .collect()
            };
            if let Ok(f) = crate::register::fit(&crop(template), &crop(warped), side, side, &o)
                && f.residual < 0.3
            {
                out.push((
                    (x0 + side / 2) as f64,
                    (y0 + side / 2) as f64,
                    f.transform.m[2],
                    f.transform.m[5],
                    f.residual,
                ));
            }
        }
    }
    out
}

/// A transform as scale, turn, and where it takes the middle.
fn described(t: &Transform, (w, h): (usize, usize)) -> String {
    let (dx, dy) = t.displacement((w - 1) as f64 / 2.0, (h - 1) as f64 / 2.0);
    format!(
        "scale {:+.4}% turn {:+.4} deg middle {dx:+.2},{dy:+.2}",
        (t.scale() - 1.0) * 100.0,
        t.rotation().to_degrees()
    )
}

/// Real pairs named in `GREYCARD_PAIRS` (a path list, taken two at a
/// time), each fitted both ways by the gradient alone, by the phase
/// unguarded, and by the phase as the stack runs it, guarded. Two
/// checks no blur difference moves: forward then backward should come
/// back where it started, and a grid of 512-pixel patches each fitted
/// by a shift alone should find nothing left once the frame is placed
/// (the median of what they find). With the phase's own median miss,
/// the number the guard reads.
#[test]
#[ignore = "a measurement on real pairs named in GREYCARD_PAIRS"]
fn measure_real_pairs() {
    use crate::register::{Model, Options as Fitting, fit_from, refine_by_phase_within};
    let Some(list) = std::env::var_os("GREYCARD_PAIRS") else {
        return;
    };
    let files: Vec<std::path::PathBuf> = std::env::split_paths(&list).collect();
    for (number, pair) in files.chunks(2).filter(|p| p.len() == 2).enumerate() {
        let (w, h, lum) = real_planes(pair);
        println!("== pair {number} at {w}x{h}");
        let gradient = Fitting {
            model: Model::Similarity,
            ..Default::default()
        };
        for (how, most) in [
            ("gradient", None),
            ("phase, unguarded", Some(f64::INFINITY)),
            ("phase, guarded", Some(crate::register::PHASE_MISS_MOST)),
        ] {
            let t0 = std::time::Instant::now();
            let one = |a: &[f32], b: &[f32]| -> (Transform, f32, String) {
                let fit = fit_from(a, b, w, h, &gradient, &Transform::IDENTITY).expect("a fit");
                let Some(most) = most else {
                    return (fit.transform, fit.residual, String::new());
                };
                match refine_by_phase_within(a, b, w, h, Model::Similarity, &fit.transform, most) {
                    Ok(p) => (
                        p.transform,
                        fit.residual,
                        format!(
                            "refined: {} passes, {} patches, miss {:.3} plain {:.3} effective {:.0} px at 1/{}",
                            p.passes,
                            p.patches,
                            p.agreement.miss,
                            p.agreement.plain,
                            p.agreement.effective,
                            1.0 / p.level
                        ),
                    ),
                    Err(why) => (fit.transform, fit.residual, format!("kept: {why:?}")),
                }
            };
            let (forward, residual, said) = one(&lum[0], &lum[1]);
            let seconds = t0.elapsed().as_secs_f64();
            let (backward, _, said_back) = one(&lum[1], &lum[0]);
            let round = backward
                .compose(&forward)
                .corner_distance(&Transform::IDENTITY, w, h);
            let warped = crate::register::warp_plane(
                &lum[1],
                w,
                h,
                w,
                h,
                &forward,
                crate::register::Edge::Clamp,
                crate::register::Kernel::Cubic,
            );
            let field = patch_field(&lum[0], &warped, &forward, (w, h), (11, 7), 512);
            let mut left: Vec<f64> = field.iter().map(|p| p.2.hypot(p.3)).collect();
            left.sort_by(f64::total_cmp);
            println!(
                "  {how:<17} {} (residual {residual:.3}, {seconds:.1} s) | forward then back \
                 {round:.3} px | {} patches, left median {:.3} px | {said} / back {said_back}",
                described(&forward, (w, h)),
                left.len(),
                left.get(left.len() / 2).copied().unwrap_or(f64::NAN),
            );
        }
    }
}

/// The chain over a bed under the stack's default options, which refine
/// every link by the phase.
fn placed_by_default(planes: &Planes) -> Vec<Placed> {
    let placed = place(planes, &Options::default());
    show("default", &placed, true);
    placed
}

/// Eleven frames breathing 0.15% a step, on both beds: the gradient fit
/// alone leaves the far frames 2.86 px out at the corner on the
/// texture and 1.84 px on the depth bed, a turn and a shift the defocus
/// invented, added up link by link; refined, 1.03 and 0.85.
#[test]
fn a_stacks_breathing_is_placed_to_the_corners() {
    let texture = placed_by_default(&Planes::of_bed(&Bed::new(480, 320, 11)));
    assert!(texture.iter().all(|p| p.used));
    assert!(worst(&texture) < 1.6, "{:.3} px", worst(&texture));
    // Every magnification in the right direction and within 0.1% of
    // the truth.
    for p in &texture {
        assert!(
            (p.scale - p.true_scale).abs() < 1e-3,
            "frame {}: {:.6} against {:.6}",
            p.index,
            p.scale,
            p.true_scale
        );
    }
    let shot = placed_by_default(&Planes::of_shot(&super::bed::Shot::default()));
    assert!(shot.iter().all(|p| p.used));
    assert!(worst(&shot) < 1.5, "{:.3} px", worst(&shot));
}

/// The same stack with no breathing: the gradient fit alone invents a
/// magnification of 0.39% at one end and a turn of 0.4 degrees at the
/// other, 3.1 px at the corner; refined, no frame's scale is 0.12% off
/// one and the worst corner is 1.59 px.
#[test]
fn a_stack_without_breathing_is_given_none() {
    let placed = placed_by_default(&Planes::of_bed(&Bed {
        breathing: 0.0,
        ..Bed::new(480, 320, 11)
    }));
    for p in &placed {
        assert!(
            (p.scale - 1.0).abs() < 1.5e-3,
            "frame {}: scale {:.6}",
            p.index,
            p.scale
        );
    }
    assert!(worst(&placed) < 2.0, "{:.3} px", worst(&placed));
}

/// Small stacks. Two and three frames are a large step of defocus apart,
/// past what the phase is blind to: their patches disagree (a median
/// miss of 0.5 to 1.3 px) or do not settle, the guard turns the
/// refinement away, and the placing is the gradient fit's to the bit,
/// never worse. Five frames held by hand, noisy, gain: 2.86 px at the
/// corner from the gradient fit alone, 2.09 refined.
#[test]
fn a_small_stack_is_placed_no_worse_and_five_frames_gain() {
    let gradient = {
        let mut options = Options::default();
        options.register.refine = Refine::None;
        options
    };
    for frames in [2, 3] {
        for bed in [
            Bed::new(480, 320, frames),
            Bed {
                noise: 0.01,
                shake: 3.0,
                turn: 0.1,
                ..Bed::new(480, 320, frames)
            },
            Bed {
                smeared: Some((frames / 2 + 1, 4.0)),
                ..Bed::new(480, 320, frames)
            },
        ] {
            let planes = Planes::of_bed(&bed);
            let (refined, alone) = (placed_by_default(&planes), place(&planes, &gradient));
            assert!(
                worst(&refined) <= worst(&alone) + 1e-9,
                "{frames} frames: {:.3} px refined against {:.3}",
                worst(&refined),
                worst(&alone)
            );
        }
    }
    let held = placed_by_default(&Planes::of_bed(&Bed {
        noise: 0.01,
        shake: 3.0,
        turn: 0.1,
        ..Bed::new(480, 320, 5)
    }));
    assert!(held.iter().all(|p| p.used));
    assert!(worst(&held) < 2.5, "{:.3} px", worst(&held));
}

/// One frame of eleven out of focus everywhere, by four more pixels of
/// defocus: the frames on the reference's other side are placed as if
/// it were not there (0.55 px at the worst, where the gradient fit
/// alone left 1.27), and the chain past it ends 2.06 px out, against
/// 3.31.
#[test]
fn a_frame_out_of_focus_everywhere_does_not_throw_the_chain() {
    let bed = Bed {
        smeared: Some((6, 4.0)),
        ..Bed::new(480, 320, 11)
    };
    let placed = placed_by_default(&Planes::of_bed(&bed));
    assert!(placed.iter().all(|p| p.used));
    let before = placed[..6].iter().map(|p| p.corner).fold(0.0, f64::max);
    assert!(before < 0.6, "{before:.3} px");
    assert!(worst(&placed) < 2.6, "{:.3} px", worst(&placed));
}
