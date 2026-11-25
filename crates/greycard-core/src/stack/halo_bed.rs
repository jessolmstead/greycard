//! A synthetic focus stack around a high-contrast object, for measuring
//! the halo a defocused frame's disc leaves: a disc in focus in the
//! middle frame of the stack, over a textured ground whose depth runs
//! across the frame, so the ground at the disc is in focus in that
//! frame too. Frame `k` sees the ground through a Gaussian of `ground`
//! px a step of focus from it, and the disc through a pillbox of `disc`
//! px a step, so each frame's disc is wider than the last and its rim
//! is a real edge in that frame.
//!
//! Three objects: a blown specular (six times the clip over the ground,
//! held to 1 as a frame's clip holds it), a bright one under the clip
//! (a petal's edge over a dark ground), and a dark one in front of a
//! bright ground, whose halo is the reverse.

use rayon::prelude::*;

use crate::image::WorkingImage;

/// What sits in the middle of the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Object {
    /// Light added over the ground, six times the clip, held to 1.
    Blown,
    /// A bright disc over a dark ground, under the clip: four stops.
    Bright,
    /// A dark disc in front of a bright ground: four stops under it.
    Dark,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Scene {
    pub width: usize,
    pub height: usize,
    pub frames: usize,
    pub object: Object,
    /// The disc's radius, px.
    pub radius: f64,
    /// The pillbox radius a step of focus adds to the disc.
    pub disc: f64,
    /// The Gaussian deviation a step of focus adds to the ground.
    pub ground: f64,
    /// Noise as the bed's: shot variance per unit, read deviation.
    pub shot: f64,
    pub read: f64,
}

impl Scene {
    pub fn new(object: Object, frames: usize) -> Self {
        // The same reach at any count: the end frames' disc is 10 px
        // wider than its own.
        let half = (frames / 2).max(1) as f64;
        Self {
            width: 240,
            height: 180,
            frames,
            object,
            radius: 8.0,
            disc: 10.0 / half,
            ground: 2.8 / half,
            shot: 2e-5,
            read: 5e-4,
        }
    }

    pub fn focus(&self) -> usize {
        self.frames / 2
    }

    pub fn centre(&self) -> (f64, f64) {
        ((self.width / 2) as f64, (self.height / 2) as f64)
    }

    /// The ground's level and the disc's, before any texture.
    fn levels(&self) -> (f64, f64) {
        match self.object {
            Object::Blown => (0.15, 6.0),
            Object::Bright => (0.05, 0.8),
            Object::Dark => (0.6, 0.0375),
        }
    }

    /// The ground at `(x, y)` in frame `k`, textured: five waves of 3
    /// to 21 px, a sixth of its level, faded by the frame's defocus.
    fn ground_at(&self, k: usize, x: f64, y: f64) -> f64 {
        let depth = (self.frames - 1) as f64 * x / (self.width - 1) as f64;
        let sigma = self.ground * (k as f64 - depth).abs();
        let waves: [(f64, f64, f64); 5] = [
            (3.1, 0.3, 0.0),
            (5.2, 1.9, 1.1),
            (8.3, 0.9, 2.3),
            (13.0, 2.6, 0.7),
            (21.0, 1.4, 1.9),
        ];
        let mut t = 1.0;
        for (period, angle, phase) in waves {
            let f = 1.0 / period;
            let fade = (-2.0 * std::f64::consts::PI.powi(2) * sigma * sigma * f * f).exp();
            let u = x * angle.cos() + y * angle.sin();
            t += 0.16 * fade * (std::f64::consts::TAU * f * u + phase).sin();
        }
        self.levels().0 * t
    }

    /// The share of a pixel `r` from the centre the disc covers in frame
    /// `k`: a pillbox over the disc.
    fn coverage(&self, k: usize, r: f64) -> f64 {
        let b = (self.disc * (k as f64 - self.focus() as f64).abs()).max(0.35);
        lens_area(r, self.radius, b) / (std::f64::consts::PI * b * b)
    }

    /// Frame `k`'s value at a pixel, without noise.
    pub fn clean(&self, k: usize, x: usize, y: usize) -> f64 {
        let (fx, fy) = (x as f64, y as f64);
        let (cx, cy) = self.centre();
        let a = self.coverage(k, ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt());
        let g = self.ground_at(k, fx, fy);
        let level = self.levels().1;
        match self.object {
            Object::Blown => (g + level * a).min(1.0),
            Object::Bright | Object::Dark => a * level + (1.0 - a) * g,
        }
    }

    fn noise(&self, k: usize, x: usize, y: usize, v: f64) -> f64 {
        let seed = 5000 + k as u64;
        let a = hash(x as i64, y as i64, seed).max(1e-12);
        let b = hash(x as i64, y as i64, seed + 7919);
        let g = (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos();
        g * (self.shot * v.max(0.0) + self.read * self.read).sqrt()
    }

    fn image(&self, f: impl Fn(usize, usize) -> f64 + Sync) -> WorkingImage {
        let w = self.width;
        let data: Vec<f32> = (0..w * self.height)
            .into_par_iter()
            .flat_map_iter(|i| {
                let v = f(i % w, i / w) as f32;
                [v, v, v]
            })
            .collect();
        WorkingImage::from_data(w, self.height, data).expect("a frame")
    }

    /// The stack, noisy; a blown frame is held to 1 after the noise, as
    /// a sensor's clip holds it.
    pub fn frames(&self) -> Vec<WorkingImage> {
        let held = self.object == Object::Blown;
        (0..self.frames)
            .map(|k| {
                self.image(|x, y| {
                    let v = self.clean(k, x, y);
                    let v = v + self.noise(k, x, y, v);
                    if held { v.min(1.0) } else { v }
                })
            })
            .collect()
    }

    /// The focused frame without noise: the disc and the ground beside
    /// it both sharp, what the merge is after about the disc.
    pub fn truth(&self) -> WorkingImage {
        self.image(|x, y| self.clean(self.focus(), x, y))
    }
}

/// The area two discs of radii `r1` and `r2` share, `d` apart.
fn lens_area(d: f64, r1: f64, r2: f64) -> f64 {
    use std::f64::consts::PI;
    if d >= r1 + r2 {
        return 0.0;
    }
    if d <= (r1 - r2).abs() {
        return PI * r1.min(r2).powi(2);
    }
    let a = ((d * d + r1 * r1 - r2 * r2) / (2.0 * d * r1)).clamp(-1.0, 1.0);
    let b = ((d * d + r2 * r2 - r1 * r1) / (2.0 * d * r2)).clamp(-1.0, 1.0);
    let k = ((-d + r1 + r2) * (d + r1 - r2) * (d - r1 + r2) * (d + r1 + r2)).max(0.0);
    r1 * r1 * a.acos() + r2 * r2 * b.acos() - 0.5 * k.sqrt()
}

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

/// The mean of a picture's green over rings 1 px wide about the centre.
pub(super) fn radial(image: &WorkingImage, to: usize) -> Vec<f64> {
    let (w, h) = (image.width, image.height);
    let (cx, cy) = ((w / 2) as f64, (h / 2) as f64);
    let mut sum = vec![0.0f64; to];
    let mut n = vec![0usize; to];
    for y in 0..h {
        for x in 0..w {
            let r = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt() as usize;
            if r < to {
                sum[r] += image.pixel(x, y)[1] as f64;
                n[r] += 1;
            }
        }
    }
    sum.iter()
        .zip(&n)
        .map(|(s, &n)| s / n.max(1) as f64)
        .collect()
}

/// What a merge made of the disc, against the focused frame.
#[derive(Debug, Clone, Copy)]
pub(super) struct Halo {
    /// The steepest fall of the radial profile's log at the disc's own
    /// edge (within 2 px of it), stops a pixel.
    pub edge: f64,
    /// The steepest fall or rise anywhere from 2 px outside the disc's
    /// edge to 40 px out: a contour the focused frame does not have.
    pub stray: f64,
    /// The radius of that contour.
    pub stray_at: usize,
    /// The worst departure of the radial profile from the truth's, in
    /// stops, from 2 px outside the disc to 40 px out: a halo, dark
    /// (negative) or light.
    pub outside: f64,
    /// The same inside the disc, up to 2 px from its edge.
    pub inside: f64,
    /// The darkest ring outside the disc over the ground's level (the
    /// measure §743 took).
    pub darkest: f64,
    /// The ground's fine texture kept in a ring from 4 to 24 px outside
    /// the disc: the merge's fine band projected on the truth's.
    pub kept: f64,
}

/// The fine band of a picture's green, as the bed takes it.
fn band(image: &WorkingImage) -> Vec<f32> {
    let (w, h) = (image.width, image.height);
    let g: Vec<f32> = image.data.chunks(3).map(|p| p[1]).collect();
    let a = super::bed::gauss(&g, w, h, 0.7);
    let b = super::bed::gauss(&g, w, h, 2.5);
    a.iter().zip(&b).map(|(a, b)| a - b).collect()
}

pub(super) fn measure(scene: &Scene, merged: &WorkingImage, truth: &WorkingImage) -> Halo {
    let to = 64;
    let p = radial(merged, to);
    let t = radial(truth, to);
    let r0 = scene.radius as usize;
    let slope = |q: &[f64], r: usize| (q[r].max(1e-6) / q[r + 1].max(1e-6)).log2();
    let edge = (r0 - 2..=r0 + 1)
        .map(|r| slope(&p, r).abs())
        .fold(0.0, f64::max);
    let (stray_at, stray) = (r0 + 2..40)
        .map(|r| (r, slope(&p, r).abs()))
        .fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
    let off = |r: usize| (p[r].max(1e-6) / t[r].max(1e-6)).log2();
    let worst = |rs: std::ops::Range<usize>| {
        rs.map(off)
            .fold(0.0f64, |a, b| if b.abs() > a.abs() { b } else { a })
    };
    let ground = p[48..to].iter().sum::<f64>() / (to - 48) as f64;
    let darkest = p[r0 + 1..40].iter().copied().fold(f64::INFINITY, f64::min) / ground;
    let (mb, tb) = (band(merged), band(truth));
    let (cx, cy) = scene.centre();
    let (mut mt, mut tt) = (0.0f64, 0.0f64);
    for y in 0..scene.height {
        for x in 0..scene.width {
            let r = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
            if r >= scene.radius + 4.0 && r < scene.radius + 24.0 {
                let i = y * scene.width + x;
                mt += mb[i] as f64 * tb[i] as f64;
                tt += (tb[i] as f64).powi(2);
            }
        }
    }
    Halo {
        edge,
        stray,
        stray_at,
        outside: worst(r0 + 2..40),
        inside: worst(0..r0 - 2),
        darkest,
        kept: mt / tt,
    }
}

/// The frames merged where they are, the middle the reference: the
/// blend alone, with no fit to stand between the scene and it.
pub(super) fn placed(frames: &[WorkingImage], options: &super::Options) -> WorkingImage {
    let n = frames.len();
    let places = vec![Some(crate::register::Transform::IDENTITY); n];
    let (w, h) = (frames[0].width, frames[0].height);
    let (merged, _) = super::stack_camera_placed(
        n / 2,
        &places,
        true,
        |k| crate::image::CameraImage::from_data(w, h, frames[k].data.clone()),
        |_| {},
        options,
    )
    .expect("a stack");
    WorkingImage::from_data(w, h, merged.data).expect("a frame")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::CameraImage;
    use crate::stack::{self, Options};

    fn line(name: &str, h: &Halo) {
        println!(
            "| {name} | {:.2} | {:.2} at {} | {:+.2} | {:+.2} | {:.3} | {:.2} |",
            h.edge, h.stray, h.stray_at, h.outside, h.inside, h.darkest, h.kept
        );
    }

    /// The merge as it was before the spill was held down.
    fn unspilled() -> Options {
        Options {
            spill: false,
            ..Options::default()
        }
    }

    /// The merge and the focused frame, measured, with the merge before
    /// the spill was held down printed beside them.
    fn merged(scene: &Scene) -> (Halo, Halo) {
        let frames = scene.frames();
        let truth = scene.truth();
        let after = measure(scene, &placed(&frames, &Options::default()), &truth);
        let focused = measure(scene, &frames[scene.focus()], &truth);
        println!("{:?}, {} frames", scene.object, scene.frames);
        let before = measure(scene, &placed(&frames, &unspilled()), &truth);
        line("before", &before);
        line("after", &after);
        line("focused frame", &focused);
        (after, focused)
    }

    /// A blown specular, five frames (no prior) and nine: before, the
    /// defocused frames' rims stood as contours outside the disc (1.23
    /// and 0.44 stops a pixel), its own edge fell to 1.14 and 0.83, and
    /// a ring outside it went to 0.53 and 0.61 of the ground. Now one
    /// edge, near the focused frame's in steepness, no ring it lacks,
    /// and the ground beside the disc kept from the frame focused on it.
    #[test]
    fn a_blown_specular_keeps_one_edge_and_no_ring() {
        for n in [5, 9] {
            let (got, focused) = merged(&Scene::new(Object::Blown, n));
            assert!(
                got.stray < 0.15,
                "{n}: a contour of {:.2} at {} px",
                got.stray,
                got.stray_at
            );
            assert!(got.edge > 0.85 * focused.edge, "{n}: edge {:.2}", got.edge);
            assert!(
                got.darkest > 0.95 * focused.darkest,
                "{n}: a ring at {:.3} of the ground",
                got.darkest
            );
            assert!(
                got.outside.abs() < 0.15,
                "{n}: {:+.2} stops outside",
                got.outside
            );
            assert!(got.kept > 0.95, "{n}: texture kept {:.2}", got.kept);
        }
    }

    /// A stack with no clip merges as it did with nothing held, to the
    /// bit: a bright disc under the clip keeps its glow (0.16 stops at
    /// nine frames), and a dark disc's middle goes black.
    #[test]
    fn a_stack_without_a_clip_merges_as_before() {
        for object in [Object::Bright, Object::Dark] {
            let frames = Scene::new(object, 9).frames();
            let held = placed(&frames, &Options::default());
            let before = placed(&frames, &unspilled());
            assert!(held.data == before.data, "{object:?}");
        }
    }

    /// The same nine through the whole merge, each frame placed by the
    /// chain's fit: the spill gathered in the placing pass.
    #[test]
    fn a_blown_specular_placed_by_the_chain() {
        let scene = Scene::new(Object::Blown, 9);
        let frames = scene.frames();
        let truth = scene.truth();
        let (merged, report) = stack::stack(&frames, &Options::default()).expect("a stack");
        assert_eq!(report.used(), 9, "{:#?}", report.frames);
        let got = measure(&scene, &merged, &truth);
        let focused = measure(&scene, &frames[scene.focus()], &truth);
        line("chained", &got);
        assert!(
            got.stray < 0.15,
            "a contour of {:.2} at {} px",
            got.stray,
            got.stray_at
        );
        assert!(
            got.darkest > 0.95 * focused.darkest,
            "a ring at {:.3}",
            got.darkest
        );
    }

    /// A frame of another scene, blown over a broad patch, put among a
    /// stack without a clip: the fit drops it, and it wakes nothing. Had
    /// its clip woken the spill as it was read, the frames placed before
    /// it were read again for a spill a stack made again, which reads
    /// only the placed, never gathers.
    #[test]
    fn a_dropped_frame_at_its_clip_wakes_nothing() {
        let scene = Scene::new(Object::Bright, 5);
        let (w, h) = (scene.width, scene.height);
        let mut camera: Vec<CameraImage> = scene
            .frames()
            .into_iter()
            .map(|f| CameraImage::from_data(w, h, f.data).unwrap())
            .collect();
        let other: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                let v = if x > 150 && y < 60 {
                    1.0
                } else {
                    0.05 + 0.3 * (((x / 3) * 7919 + (y / 3) * 104_729) % 1013) as f32 / 1013.0
                };
                [v, v, v]
            })
            .collect();
        camera.insert(1, CameraImage::from_data(w, h, other).unwrap());
        let options = Options::default();
        let reads = std::cell::RefCell::new(vec![0usize; camera.len()]);
        let (first, report) = stack::stack_camera_from(
            camera.len(),
            |k| {
                reads.borrow_mut()[k] += 1;
                Ok::<_, crate::error::Error>(camera[k].clone())
            },
            |_| {},
            &options,
        )
        .expect("first");
        assert!(!report.frames[1].used(), "{:#?}", report.frames);
        assert!(!report.clipped);
        let want: Vec<usize> = report
            .frames
            .iter()
            .map(|f| 1 + f.used() as usize)
            .collect();
        assert_eq!(reads.into_inner(), want);
        let placed: Vec<_> = report
            .frames
            .iter()
            .map(|f| f.used().then_some(f.transform))
            .collect();
        let (again, _) = stack::stack_camera_placed(
            report.reference,
            &placed,
            report.clipped,
            |k| Ok::<_, crate::error::Error>(camera[k].clone()),
            |_| {},
            &options,
        )
        .expect("again");
        assert!(
            again
                .data
                .iter()
                .map(|v| v.to_bits())
                .eq(first.data.iter().map(|v| v.to_bits()))
        );
    }

    /// A stack under the prior's eight frames made again from its
    /// places: the spill gathered again, each frame read twice, and the
    /// picture the same to the bit; without the spill it is another
    /// picture. Its defocused discs are wider than the fit allows, so
    /// the fit is let off.
    #[test]
    fn a_short_stack_placed_again_hears_its_spill_again() {
        let scene = Scene::new(Object::Blown, 5);
        let (w, h) = (scene.width, scene.height);
        let camera: Vec<CameraImage> = scene
            .frames()
            .into_iter()
            .map(|f| CameraImage::from_data(w, h, f.data).unwrap())
            .collect();
        let options = Options {
            max_residual: 2.0,
            ..Options::default()
        };
        let (first, report) = stack::stack_camera(&camera, &options).expect("first");
        let used = report.used();
        assert!((2..stack::prior::MIN_FRAMES).contains(&used), "{used}");
        let off = Options {
            spill: false,
            ..options
        };
        let (unspilled, _) = stack::stack_camera(&camera, &off).expect("unspilled");
        assert!(first.data.iter().zip(&unspilled.data).any(|(a, b)| a != b));
        let placed: Vec<_> = report
            .frames
            .iter()
            .map(|f| f.used().then_some(f.transform))
            .collect();
        let mut reads = vec![0usize; camera.len()];
        let mut steps = Vec::new();
        assert!(report.clipped);
        let (again, _) = stack::stack_camera_placed(
            report.reference,
            &placed,
            report.clipped,
            |k| {
                reads[k] += 1;
                Ok::<_, crate::error::Error>(camera[k].clone())
            },
            |s| steps.push(s),
            &options,
        )
        .expect("again");
        assert!(
            again
                .data
                .iter()
                .map(|v| v.to_bits())
                .eq(first.data.iter().map(|v| v.to_bits()))
        );
        let twice: Vec<usize> = placed.iter().map(|p| 2 * p.is_some() as usize).collect();
        assert_eq!(reads, twice);
        assert_eq!(
            steps.iter().filter(|s| **s == stack::Step::Placing).count(),
            1
        );
    }

    /// Every object at five, nine and thirteen frames: before, after, at
    /// a selectivity of 0, and the focused frame alone. A measurement:
    /// `cargo test -p greycard-core glow_measure -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement"]
    fn glow_measure() {
        for object in [Object::Blown, Object::Bright, Object::Dark] {
            for n in [5usize, 9, 13] {
                let scene = Scene::new(object, n);
                let frames = scene.frames();
                let truth = scene.truth();
                println!("{object:?}, {n} frames");
                println!(
                    "| result | edge stops/px | stray contour | halo outside | inside | darkest / ground | texture kept |"
                );
                println!("|---|---|---|---|---|---|---|");
                let t = std::time::Instant::now();
                let before = placed(&frames, &unspilled());
                let took_before = t.elapsed().as_secs_f64();
                line("before", &measure(&scene, &before, &truth));
                let t = std::time::Instant::now();
                let after = placed(&frames, &Options::default());
                let took = t.elapsed().as_secs_f64();
                line("after", &measure(&scene, &after, &truth));
                let flat = placed(
                    &frames,
                    &Options {
                        selectivity: 0.0,
                        ..Options::default()
                    },
                );
                line("selectivity 0", &measure(&scene, &flat, &truth));
                line(
                    "focused frame",
                    &measure(&scene, &frames[scene.focus()], &truth),
                );
                println!("  merge {took_before:.2} s before, {took:.2} s after");
            }
        }
    }

    /// Crops about the disc, before | after | the focused frame, each
    /// object at nine frames, four times up, into `GREYCARD_GLOW_OUT`:
    /// at 0 EV and, for the blown one, 3 EV down, gamma 2.2.
    #[test]
    #[ignore = "writes pictures"]
    fn glow_crops() {
        let Some(out) = std::env::var_os("GREYCARD_GLOW_OUT") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        let side = 72usize;
        let scale = 4usize;
        for (object, name) in [
            (Object::Blown, "clipped"),
            (Object::Bright, "unclipped"),
            (Object::Dark, "dark"),
        ] {
            let scene = Scene::new(object, 9);
            let frames = scene.frames();
            let shots = [
                placed(&frames, &unspilled()),
                placed(&frames, &Options::default()),
                frames[scene.focus()].clone(),
            ];
            let (x0, y0) = (scene.width / 2 - side / 2, scene.height / 2 - side / 2);
            let stops: &[f32] = if object == Object::Blown {
                &[0.0, -3.0]
            } else {
                &[0.0]
            };
            for &ev in stops {
                let gap = 4 * scale;
                let mut sheet = image::GrayImage::new(
                    (3 * side * scale + 2 * gap) as u32,
                    (side * scale) as u32,
                );
                for (i, shot) in shots.iter().enumerate() {
                    for y in 0..side * scale {
                        for x in 0..side * scale {
                            let v = shot.pixel(x0 + x / scale, y0 + y / scale)[1];
                            let v = (v * ev.exp2()).clamp(0.0, 1.0).powf(1.0 / 2.2);
                            sheet.put_pixel(
                                (i * (side * scale + gap) + x) as u32,
                                y as u32,
                                image::Luma([(v * 255.0).round() as u8]),
                            );
                        }
                    }
                }
                let file = format!("{name}-before-after-focused-{}ev.png", ev as i32);
                sheet.save(out.join(file)).expect("a crop written");
            }
        }
    }
}
