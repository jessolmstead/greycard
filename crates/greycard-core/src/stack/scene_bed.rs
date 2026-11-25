//! Scenes with a clip in reach that a defocused bright edge's spill
//! (`super::spill`) must leave alone: bright specks by a blown sky, deep
//! shadows under heavy noise by a blown region, dark hairs over a
//! clipped ground, small blown speculars in focus, and a blown specular
//! over strong texture. Each is merged with the spill held down and not,
//! and measured against the scene in focus everywhere: the fine texture
//! kept, the local level's error in stops (its deviation, worst, and
//! bias), and the noise. A scene with no clip merges as it did, to the
//! bit (`halo_bed`), so none is here. Written in review, adopted as the
//! spill's own bed.

use rayon::prelude::*;

use super::{Options, bed, halo_bed, spill};
use crate::image::WorkingImage;
use crate::register::{self, Transform};

fn hash(ix: i64, iy: i64, seed: u64) -> f64 {
    let mut h = (ix as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(seed.wrapping_mul(0xD6E8_FEB8_6659_FD93));
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h >> 11) as f64 / (1u64 << 53) as f64
}
fn gn(x: usize, y: usize, seed: u64) -> f64 {
    let a = hash(x as i64, y as i64, seed).max(1e-12);
    let b = hash(x as i64, y as i64, seed + 7919);
    (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
}

fn blur(p: &[f32], w: usize, h: usize, s: f64) -> Vec<f32> {
    if s < 0.25 {
        p.to_vec()
    } else {
        bed::gauss(p, w, h, s)
    }
}

#[derive(Clone)]
struct Layer {
    plane: Vec<f32>,
    mask: Vec<f32>,
    depth: f64,
}

/// Back to front, focus at `k`, `step` px of sigma a frame step.
fn compose(layers: &[Layer], k: f64, step: f64, w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    for l in layers {
        let s = step * (k - l.depth).abs();
        let pm: Vec<f32> = l.plane.iter().zip(&l.mask).map(|(p, m)| p * m).collect();
        let a = blur(&l.mask, w, h, s);
        let f = blur(&pm, w, h, s);
        out = out
            .iter()
            .zip(&a)
            .zip(&f)
            .map(|((o, a), f)| o * (1.0 - a) + f)
            .collect();
    }
    out
}

/// Waves of 3 to 31 px, `amp` of the level.
fn texture(w: usize, h: usize, level: f32, amp: f32, seed: f64) -> Vec<f32> {
    let waves: [(f64, f64, f64); 6] = [
        (3.1, 0.3, 0.0),
        (5.2, 1.9, 1.1),
        (8.3, 0.9, 2.3),
        (13.0, 2.6, 0.7),
        (21.0, 1.4, 1.9),
        (31.0, 0.6, 0.4),
    ];
    (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            let mut t = 0.0;
            for (p, a, ph) in waves {
                let u = x * (a + seed).cos() + y * (a + seed).sin();
                t += (std::f64::consts::TAU * u / p + ph + seed).sin();
            }
            level * (1.0 + amp * (t / 2.5) as f32).max(0.05)
        })
        .collect()
}

fn disc_mask(w: usize, h: usize, centres: &[(f64, f64)], r: f64) -> Vec<f32> {
    let mut m = vec![0.0f32; w * h];
    for &(cx, cy) in centres {
        let (x0, x1) = (
            (cx - r - 2.0).max(0.0) as usize,
            ((cx + r + 2.0) as usize).min(w - 1),
        );
        let (y0, y1) = (
            (cy - r - 2.0).max(0.0) as usize,
            ((cy + r + 2.0) as usize).min(h - 1),
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
                let v = (r + 0.5 - d).clamp(0.0, 1.0) as f32;
                let i = y * w + x;
                m[i] = m[i].max(v);
            }
        }
    }
    m
}

/// Lines `width` px wide, `count` of them, random.
fn hairs(w: usize, h: usize, count: usize, width: f64, seed: u64) -> Vec<f32> {
    let lines: Vec<(f64, f64, f64)> = (0..count)
        .map(|i| {
            let x = hash(i as i64, 1, seed) * w as f64;
            let y = hash(i as i64, 2, seed) * h as f64;
            let a = hash(i as i64, 3, seed) * std::f64::consts::PI;
            (x, y, a)
        })
        .collect();
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            let mut m = 0.0f64;
            for &(lx, ly, a) in &lines {
                let d = ((x - lx) * a.sin() - (y - ly) * a.cos()).abs();
                let along = ((x - lx) * a.cos() + (y - ly) * a.sin()).abs();
                if along < 60.0 {
                    m = m.max((width / 2.0 + 0.5 - d).clamp(0.0, 1.0));
                }
            }
            m as f32
        })
        .collect()
}

#[derive(Clone)]
struct Scene {
    name: String,
    w: usize,
    h: usize,
    n: usize,
    step: f64,
    layers: Vec<Layer>,
    shot: f64,
    read: f64,
    clip: Option<f32>,
    /// Per frame: gain (stops), vignette at the corner (stops), shift px, scale.
    gain: Vec<f64>,
    vignette: Vec<f64>,
    shift: Vec<(f64, f64)>,
    scale: Vec<f64>,
    /// A per-frame override of the layers (a moved subject).
    moved: Option<(usize, usize, f64)>, // (frame, layer, dx)
}

impl Scene {
    fn new(name: &str, w: usize, h: usize, n: usize, step: f64, layers: Vec<Layer>) -> Self {
        Scene {
            name: name.into(),
            w,
            h,
            n,
            step,
            layers,
            shot: 2e-5,
            read: 5e-4,
            clip: Some(1.0),
            gain: vec![0.0; n],
            vignette: vec![0.0; n],
            shift: vec![(0.0, 0.0); n],
            scale: vec![1.0; n],
            moved: None,
        }
    }
    fn clean(&self, k: usize) -> Vec<f32> {
        let (w, h) = (self.w, self.h);
        let mut layers = self.layers.clone();
        if let Some((f, l, dx)) = self.moved
            && f == k
        {
            let shiftp = |p: &[f32]| -> Vec<f32> {
                (0..w * h)
                    .map(|i| {
                        let x = (i % w) as f64 - dx;
                        let xi = x.round().clamp(0.0, (w - 1) as f64) as usize;
                        p[(i / w) * w + xi]
                    })
                    .collect()
            };
            layers[l].plane = shiftp(&layers[l].plane);
            layers[l].mask = shiftp(&layers[l].mask);
        }
        let c = compose(&layers, k as f64, self.step, w, h);
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let rmax2 = cx * cx + cy * cy;
        let (sx, sy) = self.shift[k];
        let s = self.scale[k];
        let g = 2f64.powf(self.gain[k]);
        let v = self.vignette[k];
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f64, (i / w) as f64);
                // Sample the scene where this pixel looks.
                let (u, t) = (cx + (x - cx) / s - sx, cy + (y - cy) / s - sy);
                let u = u.clamp(0.0, (w - 1) as f64 - 1e-6);
                let t = t.clamp(0.0, (h - 1) as f64 - 1e-6);
                let (iu, it) = (u as usize, t as usize);
                let (fu, ft) = ((u - iu as f64) as f32, (t - it as f64) as f32);
                let at = |a: usize, b: usize| c[b.min(h - 1) * w + a.min(w - 1)];
                let top = at(iu, it) + fu * (at(iu + 1, it) - at(iu, it));
                let bot = at(iu, it + 1) + fu * (at(iu + 1, it + 1) - at(iu, it + 1));
                let val = (top + ft * (bot - top)) as f64;
                let r2 = ((x - cx).powi(2) + (y - cy).powi(2)) / rmax2;
                val * g * 2f64.powf(-v * r2)
            })
            .map(|v| v as f32)
            .collect()
    }
    fn frames(&self) -> Vec<WorkingImage> {
        (0..self.n)
            .map(|k| {
                let c = self.clean(k);
                let data: Vec<f32> = c
                    .iter()
                    .enumerate()
                    .flat_map(|(i, &v)| {
                        let (x, y) = (i % self.w, i / self.w);
                        let vv = v as f64;
                        let nz = gn(x, y, 9000 + k as u64 * 31)
                            * (self.shot * vv.max(0.0) + self.read * self.read).sqrt();
                        let mut o = (vv + nz) as f32;
                        if let Some(c) = self.clip {
                            o = o.min(c);
                        }
                        [o, o, o]
                    })
                    .collect();
                WorkingImage::from_data(self.w, self.h, data).unwrap()
            })
            .collect()
    }
    fn truth(&self) -> Vec<f32> {
        let mut t = compose(&self.layers, 0.0, 0.0, self.w, self.h);
        if let Some(c) = self.clip {
            for v in &mut t {
                *v = v.min(c);
            }
        }
        t
    }
}

fn green(im: &WorkingImage) -> Vec<f32> {
    im.data.chunks(3).map(|p| p[1]).collect()
}

fn band(g: &[f32], w: usize, h: usize) -> Vec<f32> {
    let a = bed::gauss(g, w, h, 0.7);
    let b = bed::gauss(g, w, h, 2.5);
    a.iter().zip(&b).map(|(a, b)| a - b).collect()
}

/// Fraction of (quarter sample, frame) held under 0.5, per region, and
/// overall; by replaying the spill's gather with identity places.
fn held(frames: &[WorkingImage], regions: &[Vec<bool>]) -> (f64, Vec<f64>) {
    let n = frames.len();
    let (w, h) = (frames[0].width, frames[0].height);
    let opts = Options::default();
    let coarse = |k: usize, ceil: Option<f32>| {
        let lum = super::luminance_of(&frames[k].data, super::CAMERA_LUMA);
        let log = register::log_plane(&lum, opts.register.floor_stops);
        let e = crate::sharpness::laplacian_energy(&log, w, h);
        spill::coarse(&log, &e, w, h, ceil, spill::clips(&log, w, h))
    };
    let mut g = spill::Gathered::new(w, h);
    let order = std::iter::once(n / 2).chain(super::chain_orders(n, n / 2).into_iter().flatten());
    for k in order {
        let _ = g.add(&coarse(k, None), &Transform::IDENTITY);
    }
    let s = g.finish();
    let (qw, qh) = (w.div_ceil(4), h.div_ceil(4));
    let mut all = 0usize;
    let mut per = vec![(0usize, 0usize); regions.len()];
    for k in 0..n {
        let m = s
            .multiplier(&coarse(k, Some(s.ceiling())), &Transform::IDENTITY)
            .unwrap_or_else(|| vec![1.0; qw * qh]);
        for qy in 0..qh {
            for qx in 0..qw {
                let hd = m[qy * qw + qx] < 0.5;
                all += hd as usize;
                let i = (qy * 4).min(h - 1) * w + (qx * 4).min(w - 1);
                for (r, p) in regions.iter().zip(per.iter_mut()) {
                    if r[i] {
                        p.0 += hd as usize;
                        p.1 += 1;
                    }
                }
            }
        }
    }
    (
        all as f64 / (n * qw * qh) as f64,
        per.iter()
            .map(|&(a, b)| a as f64 / b.max(1) as f64)
            .collect(),
    )
}

struct M {
    kept: f64,
    rms: f64,
    worst: f64,
    bias: f64,
    noise: f64,
}

fn metrics(merged: &[f32], truth: &[f32], w: usize, h: usize, region: &[bool]) -> M {
    let (bm, bt) = (band(merged, w, h), band(truth, w, h));
    let lm = bed::gauss(merged, w, h, 3.0);
    let lt = bed::gauss(truth, w, h, 3.0);
    let (mut mt, mut tt, mut se, mut n, mut worst, mut sb) = (0.0, 0.0, 0.0, 0usize, 0.0f64, 0.0);
    let mut ne = 0.0;
    for i in 0..w * h {
        if !region[i] {
            continue;
        }
        mt += bm[i] as f64 * bt[i] as f64;
        tt += (bt[i] as f64).powi(2);
        let e = ((lm[i].max(1e-5) / lt[i].max(1e-5)) as f64).log2();
        se += e * e;
        sb += e;
        if e.abs() > worst.abs() {
            worst = e;
        }
        let d = (merged[i] - truth[i]) as f64 - (lm[i] - lt[i]) as f64;
        ne += d * d;
        n += 1;
    }
    let n = n.max(1) as f64;
    M {
        kept: mt / tt.max(1e-20),
        rms: (se / n).sqrt(),
        worst,
        bias: sb / n,
        noise: (ne / n).sqrt(),
    }
}

fn run(scene: &Scene, regions: &[(&str, Vec<bool>)], chain: bool) {
    let frames = scene.frames();
    let truth = scene.truth();
    let (w, h) = (scene.w, scene.h);
    let merge = |o: &Options| -> Vec<f32> {
        if chain {
            green(&super::stack(&frames, o).expect("stack").0)
        } else {
            green(&halo_bed::placed(&frames, o))
        }
    };
    let on = merge(&Options::default());
    let off = merge(&Options {
        spill: false,
        ..Options::default()
    });
    let rs: Vec<Vec<bool>> = regions.iter().map(|r| r.1.clone()).collect();
    let (all, per) = held(&frames, &rs);
    println!(
        "## {} (n={}) held overall {:.1}%",
        scene.name,
        scene.n,
        all * 100.0
    );
    for ((name, r), p) in regions.iter().zip(&per) {
        let a = metrics(&off, &truth, w, h, r);
        let b = metrics(&on, &truth, w, h, r);
        println!(
            "  {name:<14} kept {:.3}->{:.3}  rms {:.3}->{:.3}  worst {:+.3}->{:+.3}  bias {:+.3}->{:+.3}  noise {:.2e}->{:.2e}  held {:.1}%",
            a.kept,
            b.kept,
            a.rms,
            b.rms,
            a.worst,
            b.worst,
            a.bias,
            b.bias,
            a.noise,
            b.noise,
            p * 100.0
        );
    }
}

fn region(w: usize, h: usize, f: impl Fn(usize, usize) -> bool) -> Vec<bool> {
    (0..w * h).map(|i| f(i % w, i / w)).collect()
}

fn ones(w: usize, h: usize) -> Vec<f32> {
    vec![1.0; w * h]
}

const W: usize = 256;
const H: usize = 192;

fn grid(x0: f64, x1: f64, y0: f64, y1: f64, sp: f64) -> Vec<(f64, f64)> {
    let mut v = vec![];
    let mut y = y0;
    let mut r = 0;
    while y < y1 {
        let mut x = x0 + if r % 2 == 1 { sp / 2.0 } else { 0.0 };
        while x < x1 {
            v.push((x, y));
            x += sp;
        }
        y += sp;
        r += 1;
    }
    v
}

/// Bright dots in focus mid-stack on a textured ground, a far sky above.
fn dots_by_sky(sky: f32, dots: f32, n: usize) -> Scene {
    let ground = texture(W, H, 0.12, 0.25, 0.0);
    let m = disc_mask(W, H, &grid(12.0, 250.0, 60.0, 140.0, 16.0), 3.5);
    let plane: Vec<f32> = ground
        .iter()
        .zip(&m)
        .map(|(g, m)| g * (1.0 - m) + dots * m)
        .collect();
    let fmask = region(W, H, |_, y| y >= 48)
        .iter()
        .map(|&b| b as u8 as f32)
        .collect();
    let half = (n / 2) as f64;
    Scene::new(
        &format!("bright dots {dots} by sky {sky}"),
        W,
        H,
        n,
        6.0 / half,
        vec![
            Layer {
                plane: vec![sky; W * H],
                mask: ones(W, H),
                depth: (n - 1) as f64,
            },
            Layer {
                plane,
                mask: fmask,
                depth: half,
            },
        ],
    )
}

fn dots_regions() -> Vec<(&'static str, Vec<bool>)> {
    vec![
        (
            "dots 56-144",
            region(W, H, |x, y| (56..144).contains(&y) && (8..248).contains(&x)),
        ),
        (
            "dots 80-108",
            region(W, H, |x, y| (80..108).contains(&y) && (8..248).contains(&x)),
        ),
        (
            "dots 120-144",
            region(W, H, |x, y| {
                (120..144).contains(&y) && (8..248).contains(&x)
            }),
        ),
        (
            "far 150-188",
            region(W, H, |x, y| {
                (150..188).contains(&y) && (8..248).contains(&x)
            }),
        ),
    ]
}

#[test]
#[ignore = "a measurement"]
fn scenes_dots() {
    for (sky, dots) in [(0.12f32, 0.5f32), (0.9, 0.5), (2.0, 0.5), (2.0, 0.3)] {
        run(&dots_by_sky(sky, dots, 9), &dots_regions(), false);
    }
}

/// Thin hairs: dark over bright (and near the clip), bright over dark.
#[test]
#[ignore = "a measurement"]
fn scenes_hairs() {
    for (hair, ground, clip) in [
        (0.03f32, 0.6f32, 1.0f32),
        (0.03, 0.97, 1.0),
        (0.8, 0.05, 1.0),
        (0.4, 0.05, 1.0),
    ] {
        let g = texture(W, H, ground, 0.3, 0.4);
        let m = hairs(W, H, 30, 1.5, 77);
        let s = {
            let mut s = Scene::new(
                &format!("hairs {hair} over {ground}"),
                W,
                H,
                9,
                1.2,
                vec![
                    Layer {
                        plane: g,
                        mask: ones(W, H),
                        depth: 6.0,
                    },
                    Layer {
                        plane: vec![hair; W * H],
                        mask: m.clone(),
                        depth: 2.0,
                    },
                ],
            );
            s.clip = Some(clip);
            s
        };
        let on_hair = region(W, H, |x, y| {
            m[y * W + x] > 0.5 && (8..248).contains(&x) && (8..184).contains(&y)
        });
        let ground_r = region(W, H, |x, y| {
            m[y * W + x] == 0.0 && (8..248).contains(&x) && (8..184).contains(&y)
        });
        run(&s, &[("hairs", on_hair), ("ground", ground_r)], false);
    }
}

fn dots_scene(sky: f32, dots: f32, r: f64, n: usize, sky_texture: bool) -> Scene {
    let ground = texture(W, H, 0.12, 0.25, 0.0);
    let m = disc_mask(W, H, &grid(12.0, 250.0, 60.0, 140.0, 16.0), r);
    let plane: Vec<f32> = ground
        .iter()
        .zip(&m)
        .map(|(g, m)| g * (1.0 - m) + dots * m)
        .collect();
    let fmask = region(W, H, |_, y| y >= 48)
        .iter()
        .map(|&b| b as u8 as f32)
        .collect();
    let half = (n / 2) as f64;
    let skyp = if sky_texture {
        texture(W, H, sky, 0.5, 3.3)
    } else {
        vec![sky; W * H]
    };
    Scene::new(
        &format!(
            "dots {dots} r{r} by sky {sky}{}",
            if sky_texture { " (textured)" } else { "" }
        ),
        W,
        H,
        n,
        6.0 / half,
        vec![
            Layer {
                plane: skyp,
                mask: ones(W, H),
                depth: (n - 1) as f64,
            },
            Layer {
                plane,
                mask: fmask,
                depth: half,
            },
        ],
    )
}

/// A scene merged with the spill held down and not, measured over
/// `region`: (not, held).
fn both(scene: &Scene, region: &[bool], chain: bool) -> (M, M) {
    compare(scene, &scene.frames(), region, chain)
}

/// As [`both`], of `frames` made from `scene` by hand.
fn compare(scene: &Scene, frames: &[WorkingImage], region: &[bool], chain: bool) -> (M, M) {
    let truth = scene.truth();
    let merge = |o: &Options| -> Vec<f32> {
        if chain {
            green(&super::stack(frames, o).expect("stack").0)
        } else {
            green(&halo_bed::placed(frames, o))
        }
    };
    let off = merge(&Options {
        spill: false,
        ..Options::default()
    });
    let on = merge(&Options::default());
    let (w, h) = (scene.w, scene.h);
    let (a, b) = (
        metrics(&off, &truth, w, h, region),
        metrics(&on, &truth, w, h, region),
    );
    println!(
        "{} (n={}): kept {:.3}->{:.3} rms {:.3}->{:.3} worst {:+.3}->{:+.3} noise {:.2e}->{:.2e}",
        scene.name, scene.n, a.kept, b.kept, a.rms, b.rms, a.worst, b.worst, a.noise, b.noise
    );
    (a, b)
}

/// Bright specks 2 stops over the ground, sharp mid-stack, 32 to 60 px
/// from a blown sky that spills nowhere near them: as the spill was
/// first reviewed, every frame but the specks' own was darker there and
/// the sky was in reach, so the specks' frame was held down and they
/// lost 0.6 of their texture (0.41 kept). A speck is a stretch of
/// excess of its own with nothing brighter about it, and is let be.
#[test]
fn a_bright_speck_beside_a_blown_sky_is_not_held() {
    let rows = region(W, H, |x, y| (80..108).contains(&y) && (8..248).contains(&x));
    for (n, chain) in [(5, false), (9, false), (15, false), (9, true)] {
        let (off, on) = both(&dots_scene(2.0, 0.5, 3.5, n, false), &rows, chain);
        assert!(on.kept > off.kept - 0.01, "{n}: kept {:.3}", on.kept);
        assert!(on.rms < off.rms + 0.005, "{n}: rms {:.3}", on.rms);
    }
}

/// A deep shadow beside a blown region, eleven frames: under four times
/// the noise, the darkest frame was a draw from the noise's low tail,
/// every frame was held somewhere, and the few left won by a millionfold
/// (noise up by half, a blotch of 0.29 stops); with one frame at eight
/// times the others' noise, that frame was the darkest by its low draws
/// and the bar over it the quiet frames' (rms 0.16 to 0.47, the worst
/// 8.6 stops under). The bar now stands clear of the noisier's tail.
#[test]
fn a_noisy_shadow_beside_a_blown_region_is_not_held() {
    for (level, louder, one) in [
        (0.004f32, 4.0, false),
        (0.004, 1.0, true),
        (0.02, 1.0, true),
    ] {
        let ground = texture(W, H, level, 0.3, 0.5);
        let plane: Vec<f32> = ground
            .iter()
            .enumerate()
            .map(|(i, &g)| if i % W >= 170 { 2.0 } else { g })
            .collect();
        let mut s = Scene::new(
            &format!("shadow {level} by blown, noise x{louder}, one frame x8 {one}"),
            W,
            H,
            11,
            6.0 / 5.0,
            vec![Layer {
                plane,
                mask: ones(W, H),
                depth: 5.0,
            }],
        );
        s.read *= louder;
        s.shot *= louder * louder;
        let mut frames = s.frames();
        if one {
            let k = 3;
            let data: Vec<f32> = s
                .clean(k)
                .iter()
                .enumerate()
                .flat_map(|(i, &v)| {
                    let v = v as f64;
                    let sigma = (s.shot * v.max(0.0) + s.read * s.read).sqrt();
                    let o = ((v + gn(i % W, i / W, 777) * 8.0 * sigma) as f32).min(1.0);
                    [o, o, o]
                })
                .collect();
            frames[k] = WorkingImage::from_data(W, H, data).unwrap();
        }
        for x in [10..60, 110..160] {
            let rows = region(W, H, |px, y| x.contains(&px) && (8..184).contains(&y));
            let (off, on) = compare(&s, &frames, &rows, false);
            assert!(on.rms < off.rms + 0.01, "{}: rms {:.3}", s.name, on.rms);
            assert!(
                on.noise < 1.05 * off.noise,
                "{}: noise {:.2e}",
                s.name,
                on.noise
            );
            assert!(
                on.worst.abs() < off.worst.abs() + 0.05,
                "{}: worst {:+.3}",
                s.name,
                on.worst
            );
        }
    }
}

/// Dark hairs over a ground at the clip: some of the ground's texture
/// goes where the clipped ground's frames are held about the hairs, but
/// little (0.835 kept with nothing held, 0.819 held).
#[test]
fn dark_hairs_over_a_clipped_ground_keep_it() {
    let m = hairs(W, H, 30, 1.5, 77);
    let s = Scene::new(
        "hairs 0.03 over 0.97",
        W,
        H,
        9,
        1.2,
        vec![
            Layer {
                plane: texture(W, H, 0.97, 0.3, 0.4),
                mask: ones(W, H),
                depth: 6.0,
            },
            Layer {
                plane: vec![0.03; W * H],
                mask: m.clone(),
                depth: 2.0,
            },
        ],
    );
    let inner = |x: usize, y: usize| (8..248).contains(&x) && (8..184).contains(&y);
    let on_hair = region(W, H, |x, y| m[y * W + x] > 0.5 && inner(x, y));
    let ground = region(W, H, |x, y| m[y * W + x] == 0.0 && inner(x, y));
    let (off, on) = both(&s, &on_hair, false);
    assert!(on.rms < off.rms + 0.005, "hairs: rms {:.3}", on.rms);
    let (off, on) = both(&s, &ground, false);
    assert!(on.kept > off.kept - 0.03, "ground: kept {:.3}", on.kept);
    assert!(on.rms < off.rms + 0.01, "ground: rms {:.3}", on.rms);
}

/// The radial profile of `p` about each of `centres`, a value a pixel
/// of radius out to 20.
fn profile(p: &[f32], centres: &[(f64, f64)]) -> Vec<f64> {
    let mut sum = [0.0f64; 20];
    let mut n = [0usize; 20];
    for &(cx, cy) in centres {
        for y in (cy as usize - 19)..(cy as usize + 19) {
            for x in (cx as usize - 16)..(cx as usize + 16) {
                let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt() as usize;
                if d < 20 {
                    sum[d] += p[y * W + x] as f64;
                    n[d] += 1;
                }
            }
        }
    }
    sum.iter()
        .zip(&n)
        .map(|(s, &n)| s / n.max(1) as f64)
        .collect()
}

/// Small blown speculars in focus mid-stack (the frames near focus
/// blown, the far ones not), with a blown sky in reach and without: the
/// focused frame's own edge, held, is not taken for the others' spill.
/// With nothing held the edge fell from 2.38 stops a pixel to 1.73 and a
/// ring outside it went to 0.097 against the ground's 0.12.
#[test]
fn a_small_blown_specular_keeps_its_edge() {
    for (sky, r, level) in [(0.12f32, 3.0f64, 6.0f32), (2.0, 3.0, 6.0), (2.0, 5.0, 3.0)] {
        let ground = texture(W, H, 0.12, 0.25, 0.0);
        let centres: Vec<(f64, f64)> = (0..7).map(|i| (24.0 + 34.0 * i as f64, 100.0)).collect();
        let m = disc_mask(W, H, &centres, r);
        let plane: Vec<f32> = ground
            .iter()
            .zip(&m)
            .map(|(g, m)| g * (1.0 - m) + level * m)
            .collect();
        let s = Scene::new(
            &format!("blown specks r{r} at {level}x, sky {sky}"),
            W,
            H,
            9,
            1.5,
            vec![
                Layer {
                    plane: vec![sky; W * H],
                    mask: ones(W, H),
                    depth: 8.0,
                },
                Layer {
                    plane,
                    mask: region(W, H, |_, y| y >= 48)
                        .iter()
                        .map(|&b| b as u8 as f32)
                        .collect(),
                    depth: 4.0,
                },
            ],
        );
        let frames = s.frames();
        let merge = |o: &Options| profile(&green(&halo_bed::placed(&frames, o)), &centres);
        let (off, on) = (
            merge(&Options {
                spill: false,
                ..Options::default()
            }),
            merge(&Options::default()),
        );
        let steepest = |q: &[f64]| {
            (0..19)
                .map(|i| (q[i].max(1e-6) / q[i + 1].max(1e-6)).log2())
                .fold(0.0f64, f64::max)
        };
        let ring = |q: &[f64]| {
            ((r as usize + 2)..(r as usize + 8))
                .map(|i| q[i])
                .fold(f64::INFINITY, f64::min)
        };
        let core = (on[0] + on[1]) / 2.0;
        println!(
            "{}: core {core:.3}, steepest {:.2} -> {:.2}, ring {:.3} -> {:.3}",
            s.name,
            steepest(&off),
            steepest(&on),
            ring(&off),
            ring(&on)
        );
        assert!((core - 1.0).abs() < 0.01, "{}: core {core:.3}", s.name);
        assert!(steepest(&on) > steepest(&off) - 0.02, "{}: edge", s.name);
        assert!(ring(&on) > ring(&off) - 0.002, "{}: ring", s.name);
    }
}

/// A blown specular over a strongly textured ground at its own depth:
/// the noise's share of the bar, read from that texture's energy, does
/// not switch the hold off: the error of the ring about the disc falls
/// by half or more (0.069 to 0.025 at nine frames, 0.076 to 0.002 at five).
#[test]
fn a_blown_specular_over_strong_texture_is_held() {
    let (cx, cy) = (128.0, 96.0);
    let ring = region(W, H, |x, y| {
        let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
        (10.0..30.0).contains(&d)
    });
    for amp in [0.25f32, 0.9] {
        for n in [5usize, 9] {
            let half = (n / 2) as f64;
            let s = Scene::new(
                &format!("blown r8 over ground amp {amp}"),
                W,
                H,
                n,
                3.0 / half,
                vec![
                    Layer {
                        plane: texture(W, H, 0.15, amp, 0.3),
                        mask: ones(W, H),
                        depth: half,
                    },
                    Layer {
                        plane: vec![6.0; W * H],
                        mask: disc_mask(W, H, &[(cx, cy)], 8.0),
                        depth: half,
                    },
                ],
            );
            let (off, on) = both(&s, &ring, false);
            assert!(
                on.rms < 0.6 * off.rms,
                "{amp} {n}: rms {:.3} -> {:.3}",
                off.rms,
                on.rms
            );
        }
    }
}
