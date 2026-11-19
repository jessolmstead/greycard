//! The GPU demosaics (RCD, VNG4, and their dual) against their CPU
//! references, on the same mosaics.
//!
//! Without a GPU adapter each test says so and returns; it does not
//! pass in silence. (`cargo test` shows the line with `--nocapture`.)

use greycard_core::develop::DemosaicMethod;
use greycard_core::develop::dual::{DualContrast, demosaic_dual};
use greycard_core::develop::noise::NoiseModel;
use greycard_core::develop::rcd::demosaic_rcd;
use greycard_core::develop::vng4::demosaic_vng4;
use greycard_core::raw::{CfaColor, CfaPattern};
use greycard_gpu::{Context, Error};

fn context() -> Option<Context> {
    match Context::own() {
        Ok(c) => Some(c),
        Err(e) => {
            assert!(
                std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
                "the GPU demosaic has nothing to run on ({e}) and GREYCARD_REQUIRE_GPU is set"
            );
            eprintln!("SKIPPED: the GPU demosaic has nothing to run on ({e})");
            println!("SKIPPED: the GPU demosaic has nothing to run on ({e})");
            None
        }
    }
}

fn pattern(name: &str) -> CfaPattern {
    let colors = name
        .chars()
        .map(|c| match c {
            'R' => CfaColor::Red,
            'G' => CfaColor::Green,
            _ => CfaColor::Blue,
        })
        .collect();
    CfaPattern::new(2, 2, colors).unwrap()
}

const BAYER: [&str; 4] = ["RGGB", "BGGR", "GRBG", "GBRG"];

/// A mosaic with what a demosaic decides on: a ramp, a hard edge, a
/// fine checker, diagonal gratings, a highlight above one, a dark
/// corner, and noise of `noise` (a fraction of full scale), each
/// channel tinted apart.
fn mosaic(w: usize, h: usize, pattern: &CfaPattern, noise: f32, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
    };
    (0..w * h)
        .map(|i| {
            let (y, x) = (i / w, i % w);
            let c = pattern.color_at(y, x).rgb_index().unwrap();
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let base = 0.1 + 0.3 * u + 0.2 * v;
            let edge = if x > w / 2 { 0.3 } else { 0.0 };
            let checker = if (x / 3 + y / 3) % 2 == 0 { 0.05 } else { 0.0 };
            let grating = 0.08 * ((x as f32 * 0.7 + y as f32 * 0.4).sin());
            let highlight = if u > 0.75 && v < 0.2 { 0.9 } else { 0.0 };
            let dark = if u < 0.15 && v > 0.8 { 0.02 } else { 1.0 };
            let tint = [0.9, 1.0, 0.75][c];
            ((base + edge + checker + grating + highlight) * tint * dark + noise * next()).max(0.0)
        })
        .collect()
}

/// A flat mid grey under noise of 1 percent with a hard edge across
/// its right third: the flat tile the automatic threshold looks for,
/// and detail beside it.
fn flat_with_an_edge(w: usize, h: usize, pattern: &CfaPattern, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
    };
    (0..w * h)
        .map(|i| {
            let (y, x) = (i / w, i % w);
            let c = pattern.color_at(y, x).rgb_index().unwrap();
            let edge = if x > 2 * w / 3 && (x + y) % 9 < 4 {
                0.4
            } else {
                0.0
            };
            ((0.25 + edge) * [0.9, 1.0, 0.75][c] + 0.01 * next()).max(0.0)
        })
        .collect()
}

/// The largest difference, where it is, and how many values are
/// further apart than `tolerance`.
fn apart(cpu: &[f32], gpu: &[f32], w: usize, tolerance: f32) -> (f32, (usize, usize), usize) {
    assert_eq!(cpu.len(), gpu.len());
    let mut worst = (0f32, (0, 0));
    let mut over = 0;
    for (i, (a, b)) in cpu.iter().zip(gpu).enumerate() {
        let d = (a - b).abs();
        assert!(d.is_finite(), "value {i}: {a} against {b}");
        if d > worst.0 {
            worst = (d, ((i / 3) % w, (i / 3) / w));
        }
        if d > tolerance {
            over += 1;
        }
    }
    (worst.0, worst.1, over)
}

/// RCD's ratios and divisions round on the GPU as WGSL's operations
/// do, and a device may fuse a multiply and an add in its quadratic
/// forms, where the terms cancel: a few ULP of a value apart from the
/// CPU's, under a code value of sixteen bits. (Lavapipe, which fuses
/// nothing and divides as the CPU does, is the reference's to the
/// bit.) A discrimination near a half can tip the other way on such a
/// rounding, so a few values in ten thousand may step further, never
/// past [`STEP`].
const RCD_TOLERANCE: f32 = 2e-5;

/// The most a value may step where a rounding tipped a decision: a
/// tenth of what the frame's brightest detail spans here.
const STEP: f32 = 0.05;

/// The share of values that may step past the rounding's tolerance.
const STEPPED: f64 = 1e-3;

/// Whether the GPU's result is the reference's under `tolerance`, with
/// the steps a tipped decision may make; says why not.
fn holds(cpu: &[f32], gpu: &[f32], w: usize, tolerance: f32, what: &str) {
    let (worst, at, over) = apart(cpu, gpu, w, tolerance);
    println!("{what}: largest {worst:e} at {at:?}, {over} past {tolerance:e}");
    assert!(
        worst <= STEP && (over as f64) <= cpu.len() as f64 * STEPPED,
        "{what}: {over} values past {tolerance:e}, the largest {worst} at {at:?}"
    );
}

#[test]
fn rcd_matches_the_reference_on_every_layout() {
    let Some(ctx) = context() else {
        return;
    };
    for (k, name) in BAYER.iter().enumerate() {
        let p = pattern(name);
        for (w, h, noise) in [(97, 83, 0.0), (64, 48, 0.02), (130, 70, 0.005)] {
            let m = mosaic(w, h, &p, noise, 7 + k as u64);
            let cpu = demosaic_rcd(&m, w, h, &p).unwrap();
            let gpu = ctx.demosaic_rcd(&m, w, h, &p).unwrap();
            holds(
                &cpu,
                &gpu,
                w,
                RCD_TOLERANCE,
                &format!("RCD {name} {w}x{h} noise {noise}"),
            );
        }
    }
}

/// VNG4's gradients are exact on both (an absolute difference times a
/// power of two, summed in the same order), so its directions are the
/// reference's and nothing steps; its green divides by their count,
/// which can round an ULP or two apart.
const VNG4_TOLERANCE: f32 = 2e-6;

#[test]
fn vng4_matches_the_reference_on_every_layout() {
    let Some(ctx) = context() else {
        return;
    };
    for (k, name) in BAYER.iter().enumerate() {
        let p = pattern(name);
        for (w, h, noise) in [(97, 83, 0.0), (64, 48, 0.02), (130, 70, 0.005)] {
            let m = mosaic(w, h, &p, noise, 11 + k as u64);
            let cpu = demosaic_vng4(&m, w, h, &p).unwrap();
            let gpu = ctx.demosaic_vng4(&m, w, h, &p).unwrap();
            let (worst, at, over) = apart(&cpu, &gpu, w, VNG4_TOLERANCE);
            println!("VNG4 {name} {w}x{h} noise {noise}: largest {worst:e} at {at:?}");
            assert_eq!(
                over, 0,
                "VNG4 {name} {w}x{h}: {over} values past {VNG4_TOLERANCE}, largest {worst} at {at:?}"
            );
        }
    }
}

/// The dual's mask adds the cube root and the blur to RCD's roundings,
/// and a lightness on a half unit of L* may take the next entry of the
/// threshold table: a step the size of the table's there, times how
/// far RCD and VNG4 are apart at the pixel.
const DUAL_TOLERANCE: f32 = 1e-4;

#[test]
fn the_dual_matches_the_reference_under_every_threshold() {
    let Some(ctx) = context() else {
        return;
    };
    let model = NoiseModel {
        a: [4e-4, 2e-4, 5e-4],
        b: [2e-6, 1e-6, 3e-6],
    };
    for (k, name) in BAYER.iter().enumerate() {
        let p = pattern(name);
        for contrast in [
            DualContrast::Fixed(0.2),
            DualContrast::Fixed(0.0),
            DualContrast::Noise(model),
            DualContrast::Tiles,
        ] {
            // Large enough for the automatic search's 80-pixel tiles,
            // and with a flat one for it to find.
            let (w, h) = (200, 180);
            let m = if contrast == DualContrast::Tiles {
                flat_with_an_edge(w, h, &p, 23 + k as u64)
            } else {
                mosaic(w, h, &p, 0.01, 23 + k as u64)
            };
            let (cpu, stats) = demosaic_dual(&m, w, h, &p, DemosaicMethod::Rcd, contrast).unwrap();
            if contrast != DualContrast::Fixed(0.0) {
                assert!(
                    stats.flat_fraction > 0.05,
                    "{name} {contrast:?} blends: {stats:?}"
                );
            }
            let gpu = ctx.demosaic_rcd_dual(&m, w, h, &p, contrast).unwrap();
            holds(
                &cpu,
                &gpu,
                w,
                DUAL_TOLERANCE,
                &format!("RCD dual {name} {contrast:?}"),
            );
        }
    }
}

/// The AMaZE dual, the default, against the reference: AMaZE's own
/// op's roundings (`tests/amaze.rs`: a threshold of its own, the
/// Nyquist test on a zone plate among them, may fall the other way on
/// a device's fused multiply-add and its recursions carry it a few
/// cells), then the blend's. Held as that op is on a small synthetic
/// scene, relative to the reference's largest value: no more than
/// [`AMAZE_SHARE`] of the samples past 1e-3 of it, and none past a
/// tenth. (Lavapipe, which fuses nothing, is within 3e-6.)
/// The share of an AMaZE dual's samples that may be past 1e-3 of the
/// scale on a small synthetic scene; a real frame's is held tighter.
const AMAZE_SHARE: f64 = 1e-3;

#[test]
fn the_amaze_dual_matches_the_reference() {
    let Some(ctx) = context() else {
        return;
    };
    let model = NoiseModel {
        a: [4e-4, 2e-4, 5e-4],
        b: [2e-6, 1e-6, 3e-6],
    };
    for (k, name) in BAYER.iter().enumerate() {
        let p = pattern(name);
        for contrast in [
            DualContrast::Fixed(0.2),
            DualContrast::Noise(model),
            DualContrast::Tiles,
        ] {
            let (w, h) = (300, 270);
            let m = if contrast == DualContrast::Tiles {
                flat_with_an_edge(w, h, &p, 31 + k as u64)
            } else {
                mosaic(w, h, &p, 0.01, 31 + k as u64)
            };
            let (cpu, stats) =
                demosaic_dual(&m, w, h, &p, DemosaicMethod::Amaze, contrast).unwrap();
            assert!(
                stats.flat_fraction > 0.05,
                "{name} {contrast:?} blends: {stats:?}"
            );
            let gpu = ctx.demosaic_amaze_dual(&m, w, h, &p, contrast).unwrap();
            amaze_dual_holds(&cpu, &gpu, w, &format!("{name} {contrast:?}"));
        }
        // Under AMaZE's tile, its result is the reference's bilinear,
        // made on the CPU and put on the device for the blend.
        let (w, h) = (20, 20);
        for contrast in [DualContrast::Fixed(0.2), DualContrast::Noise(model)] {
            let m = mosaic(w, h, &p, 0.01, 31 + k as u64);
            let (cpu, _) = demosaic_dual(&m, w, h, &p, DemosaicMethod::Amaze, contrast).unwrap();
            let gpu = ctx.demosaic_amaze_dual(&m, w, h, &p, contrast).unwrap();
            amaze_dual_holds(&cpu, &gpu, w, &format!("{name} {w}x{h} {contrast:?}"));
        }
    }
}

/// A balanced mosaic as a frame with headroom leaves it: each channel
/// past its gain, cut at the gain times the headroom (1.15, a little
/// past Canon's 1.123 at ISO 100), a blown field at the cut beside light under it. The duals
/// hold to the reference as they do under one: nothing on the device
/// takes one as a clip. Not the RCD dual's tile threshold: no tile here
/// is flat enough, so that dual is RCD alone, and RCD on a card parts
/// from the reference within two photosites of a plateau cut at any
/// level (on the 5070 Ti, RGGB: 238 samples past 1e-3 cut at 0.8 of
/// the gains, 115 at 1, 634 at 1.15, 547 at 1.6, none uncut; every one
/// beside a cut sample). The plateau's, not the headroom's.
#[test]
fn the_duals_hold_past_one_as_the_headroom_leaves_the_mosaic() {
    let Some(ctx) = context() else {
        return;
    };
    let (headroom, gains) = (1.15f32, [2.1f32, 1.0, 1.6]);
    for (k, name) in BAYER.iter().enumerate() {
        let p = pattern(name);
        let (w, h) = (300, 270);
        let mut m = mosaic(w, h, &p, 0.01, 77 + k as u64);
        for (i, v) in m.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            let c = match p.color_at(y, x) {
                CfaColor::Red => 0,
                CfaColor::Green => 1,
                _ => 2,
            };
            // Brighter to the right, past every channel's cut there.
            let lift = 0.6 + 2.0 * x as f32 / w as f32;
            *v = (*v * lift * gains[c]).min(gains[c] * headroom);
        }
        assert!(m.iter().any(|&v| v > 2.1), "{name}: past the largest gain");
        for contrast in [DualContrast::Fixed(0.2), DualContrast::Tiles] {
            let (cpu, _) = demosaic_dual(&m, w, h, &p, DemosaicMethod::Amaze, contrast).unwrap();
            let gpu = ctx.demosaic_amaze_dual(&m, w, h, &p, contrast).unwrap();
            amaze_dual_holds(&cpu, &gpu, w, &format!("{name} past one {contrast:?}"));
        }
        let contrast = DualContrast::Fixed(0.2);
        let (cpu, _) = demosaic_dual(&m, w, h, &p, DemosaicMethod::Rcd, contrast).unwrap();
        let gpu = ctx.demosaic_rcd_dual(&m, w, h, &p, contrast).unwrap();
        real_frame_holds(&cpu, &gpu, w, &format!("{name} past one RCD"));
    }
}

/// The AMaZE dual's bound: no more than [`AMAZE_SHARE`] of the samples
/// past 1e-3 of the reference's largest value, and none past a tenth.
fn amaze_dual_holds(cpu: &[f32], gpu: &[f32], w: usize, what: &str) {
    let scale = cpu.iter().copied().fold(0f32, f32::max);
    let (worst, at, over) = apart(cpu, gpu, w, 1e-3 * scale);
    println!("AMaZE dual {what}: largest {worst:e} at {at:?}, {over} past 1e-3");
    assert!(
        worst <= 1e-1 * scale && (over as f64) <= cpu.len() as f64 * AMAZE_SHARE,
        "AMaZE dual {what}: {over} past 1e-3, the largest {worst} at {at:?}"
    );
}

/// A frame worked in bands is the frame worked whole, to the bit: the
/// halo covers everything a band's edges change.
#[test]
fn bands_are_the_whole_frame_to_the_bit() {
    let Some(ctx) = context() else {
        return;
    };
    let p = pattern("GRBG");
    let model = NoiseModel {
        a: [4e-4, 2e-4, 5e-4],
        b: [2e-6, 1e-6, 3e-6],
    };
    // An even width and an odd one; a fixed threshold, the noise
    // model's table by lightness, and the automatic search, whose
    // detail result is kept on the device from its own run.
    for ((w, h), contrast) in [
        ((72, 301), DualContrast::Fixed(0.15)),
        ((73, 301), DualContrast::Noise(model)),
        ((201, 301), DualContrast::Tiles),
    ] {
        let m = if contrast == DualContrast::Tiles {
            flat_with_an_edge(w, h, &p, 5)
        } else {
            mosaic(w, h, &p, 0.01, 5)
        };
        let run = || {
            [
                ctx.demosaic_rcd(&m, w, h, &p).unwrap(),
                ctx.demosaic_vng4(&m, w, h, &p).unwrap(),
                ctx.demosaic_rcd_dual(&m, w, h, &p, contrast).unwrap(),
                // AMaZE's result stays on the device, each band
                // starting from its own rows of it.
                ctx.demosaic_amaze_dual(&m, w, h, &p, contrast).unwrap(),
            ]
        };
        let whole = run();
        assert!(
            whole[2] != whole[0],
            "the RCD dual {contrast:?} blends at all"
        );
        assert!(
            whole[3] != ctx.demosaic_amaze(&m, w, h, &p).unwrap(),
            "the AMaZE dual {contrast:?} blends at all"
        );
        // The detail result kept on the device is in buffers of whole
        // rows; a band's copies cross from one to the next. AMaZE's
        // batches of one row of tiles copy in from every batch, its
        // ring written at every batch's offset.
        for (rows, frame_rows, tile_rows) in [
            (66, None, None),
            (80, Some(37), Some(1)),
            (131, Some(100), Some(1)),
        ] {
            ctx.limit_demosaic_bands(Some(rows));
            ctx.limit_demosaic_frame_buffers(frame_rows);
            ctx.limit_amaze_dual_batches(tile_rows);
            let banded = run();
            ctx.limit_demosaic_bands(None);
            ctx.limit_demosaic_frame_buffers(None);
            ctx.limit_amaze_dual_batches(None);
            for (op, (a, b)) in ["RCD", "VNG4", "RCD dual", "AMaZE dual"]
                .iter()
                .zip(whole.iter().zip(&banded))
            {
                assert!(a == b, "{op} {w}x{h} {contrast:?} in bands of {rows}");
            }
        }
    }
}

/// The automatic threshold is the reference's search on the GPU's
/// detail result: a few ULP there can move the flattest tile it
/// settles on, so on a real card the threshold is held to one of its
/// steps, a hundredth, of the search on the reference's detail.
#[test]
fn the_automatic_threshold_on_the_gpu_s_detail_is_within_a_step() {
    let Some(ctx) = context() else {
        return;
    };
    use greycard_core::develop::amaze::demosaic_amaze;
    use greycard_core::develop::dual::{Threshold, threshold_for};
    let at = |t: &Threshold| t.table()[50];
    for (k, name) in BAYER.iter().enumerate() {
        let p = pattern(name);
        let (w, h) = (300, 270);
        let m = flat_with_an_edge(w, h, &p, 41 + k as u64);
        for (op, cpu, gpu) in [
            (
                "RCD",
                demosaic_rcd(&m, w, h, &p).unwrap(),
                ctx.demosaic_rcd(&m, w, h, &p).unwrap(),
            ),
            (
                "AMaZE",
                demosaic_amaze(&m, w, h, &p).unwrap(),
                ctx.demosaic_amaze(&m, w, h, &p).unwrap(),
            ),
        ] {
            let (c, _) = threshold_for(DualContrast::Tiles, &cpu, w, h);
            let (g, _) = threshold_for(DualContrast::Tiles, &gpu, w, h);
            println!(
                "{op} {name}: threshold {} on the CPU's, {} on the GPU's",
                at(&c),
                at(&g)
            );
            assert!(at(&c) > 0.0, "{op} {name}: the search finds its flat tile");
            assert!(
                (at(&c) - at(&g)).abs() <= 0.01 + 1e-6,
                "{op} {name}: {} against {}",
                at(&c),
                at(&g)
            );
        }
    }
}

/// What the GPU does not take is said, for the caller to run the
/// reference: an X-Trans mosaic, a frame too small.
#[test]
fn what_it_does_not_take_is_unsupported() {
    let Some(ctx) = context() else {
        return;
    };
    let xtrans = CfaPattern::new(6, 6, vec![CfaColor::Green; 36]).unwrap();
    assert!(matches!(
        ctx.demosaic_rcd(&[0.5; 36 * 36], 36, 36, &xtrans),
        Err(Error::Unsupported(_))
    ));
    let p = pattern("RGGB");
    assert!(matches!(
        ctx.demosaic_rcd_dual(&[0.5; 8 * 8], 8, 8, &p, DualContrast::Fixed(0.2)),
        Err(Error::Unsupported(_))
    ));
}

/// A real frame's GPU result against the reference, as the AMaZE op's
/// real frames are held: every value finite, none further apart than a
/// tenth of the reference's largest value (a decision a fused
/// multiply-add tipped at a hard edge moves a value by as much as the
/// edge's own contrast), and no more than one in ten thousand past
/// 1e-3 of it. On lavapipe, which fuses nothing, RCD, VNG4, and the
/// RCD dual are the reference's to the bit or an ULP; a real card's
/// rounding tips a few of their direction and mask decisions.
fn real_frame_holds(cpu: &[f32], gpu: &[f32], w: usize, what: &str) {
    let scale = cpu.iter().copied().fold(0f32, f32::max).max(1e-6);
    let (worst, at, over) = apart(cpu, gpu, w, 1e-3 * scale);
    let (_, _, small) = apart(cpu, gpu, w, 1e-4);
    println!(
        "{what}: largest {worst:e} at {at:?} (scale {scale:.3}), {over} past 1e-3 of the scale, \
         {small} past 1e-4, of {}",
        cpu.len()
    );
    assert!(
        worst <= 1e-1 * scale && (over as f64) <= cpu.len() as f64 * 1e-4,
        "{what}: {over} past 1e-3 of the scale, the largest {worst} at {at:?}"
    );
}

/// Each Bayer frame in `GREYCARD_SAMPLES` (or only `GREYCARD_FRAME` in
/// it), prepared as the develop prepares it, through the RCD dual and
/// the AMaZE dual under the threshold the develop would give it,
/// against the reference ([`real_frame_holds`]); each line names its
/// op.
#[test]
#[ignore]
fn the_real_frames_match_the_reference() {
    let Some(ctx) = context() else {
        return;
    };
    for (name, prepared, p, contrast) in real_frames() {
        let (w, h) = (prepared.width, prepared.height);
        let s = &prepared.samples;
        let (cpu, _) = demosaic_dual(s, w, h, &p, DemosaicMethod::Rcd, contrast).unwrap();
        let gpu = ctx.demosaic_rcd_dual(s, w, h, &p, contrast).unwrap();
        real_frame_holds(&cpu, &gpu, w, &format!("{name} {w}x{h} RCD dual"));
        let (cpu, _) = demosaic_dual(s, w, h, &p, DemosaicMethod::Amaze, contrast).unwrap();
        let gpu = ctx.demosaic_amaze_dual(s, w, h, &p, contrast).unwrap();
        real_frame_holds(&cpu, &gpu, w, &format!("{name} {w}x{h} AMaZE dual"));
    }
}

/// The Bayer frames of `GREYCARD_SAMPLES`, or the one `GREYCARD_FRAME`
/// names there, prepared as the develop prepares them, with their
/// patterns and the dual's threshold the develop gives them.
fn real_frames() -> Vec<(
    String,
    greycard_core::develop::Prepared,
    CfaPattern,
    DualContrast,
)> {
    let Some(dir) = std::env::var_os("GREYCARD_SAMPLES") else {
        println!("SKIPPED: GREYCARD_SAMPLES is not set");
        return Vec::new();
    };
    let only = std::env::var("GREYCARD_FRAME").ok();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("read GREYCARD_SAMPLES")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| greycard_core::decode::is_raw_path(p))
        .filter(|p| {
            only.as_deref()
                .is_none_or(|o| p.file_name().is_some_and(|n| n == o))
        })
        .collect();
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Ok(frame) = greycard_core::decode::decode_path(&path) else {
            println!("{name}: passed over, it does not decode");
            continue;
        };
        let settings = greycard_core::develop::DevelopSettings::default();
        let prepared = greycard_core::develop::prepare(&frame, &settings, true).expect("prepares");
        let Some(p) = prepared
            .pattern
            .clone()
            .filter(greycard_core::develop::rcd::is_bayer)
        else {
            println!("{name}: passed over, not a Bayer mosaic");
            continue;
        };
        let contrast = greycard_core::develop::dual_contrast(
            &settings,
            prepared.noise.as_ref(),
            prepared.gains,
        );
        out.push((name, prepared, p, contrast));
    }
    out
}

/// One pixel of a real frame, `GREYCARD_AT` ("x,y") on `GREYCARD_FRAME`,
/// looked at closely on a 256-pixel crop around it: the mosaic's 5x5
/// there, RCD's, VNG4's, and the RCD dual's values on the CPU and the
/// GPU, and how far RCD and VNG4 are apart, which is as far as a mask
/// decision tipped there can move the dual. A difference that is a
/// rounding's on this device and a step on another is a decision; one
/// that is a step here would be the kernels' own.
#[test]
#[ignore]
fn one_pixel_looked_at() {
    let Some(ctx) = context() else {
        return;
    };
    let Ok(at) = std::env::var("GREYCARD_AT") else {
        println!("SKIPPED: GREYCARD_AT is not set");
        return;
    };
    let (x, y) = at
        .split_once(',')
        .map(|(x, y)| {
            (
                x.trim().parse::<usize>().unwrap(),
                y.trim().parse::<usize>().unwrap(),
            )
        })
        .expect("GREYCARD_AT is x,y");
    for (name, prepared, p, contrast) in real_frames() {
        let (w, h) = (prepared.width, prepared.height);
        // An even origin keeps the crop's CFA phases the frame's.
        let side = 256usize;
        let x0 = x.saturating_sub(side / 2).min(w - side) & !1;
        let y0 = y.saturating_sub(side / 2).min(h - side) & !1;
        let crop: Vec<f32> = (y0..y0 + side)
            .flat_map(|yy| prepared.samples[yy * w + x0..yy * w + x0 + side].to_vec())
            .collect();
        let (cx, cy) = (x - x0, y - y0);
        let px = |rgb: &[f32]| {
            let i = (cy * side + cx) * 3;
            [rgb[i], rgb[i + 1], rgb[i + 2]]
        };
        println!("{name} at ({x},{y}), {:?} there", p.color_at(y, x));
        for dy in -2i64..=2 {
            let row: Vec<String> = (-2i64..=2)
                .map(|dx| {
                    let v = crop[((cy as i64 + dy) as usize) * side + (cx as i64 + dx) as usize];
                    format!("{v:.4}")
                })
                .collect();
            println!("  mosaic {}", row.join(" "));
        }
        let cpu_rcd = demosaic_rcd(&crop, side, side, &p).unwrap();
        let gpu_rcd = ctx.demosaic_rcd(&crop, side, side, &p).unwrap();
        let cpu_vng = demosaic_vng4(&crop, side, side, &p).unwrap();
        let gpu_vng = ctx.demosaic_vng4(&crop, side, side, &p).unwrap();
        let (cpu_dual, _) =
            demosaic_dual(&crop, side, side, &p, DemosaicMethod::Rcd, contrast).unwrap();
        let gpu_dual = ctx
            .demosaic_rcd_dual(&crop, side, side, &p, contrast)
            .unwrap();
        println!("  RCD      CPU {:?} GPU {:?}", px(&cpu_rcd), px(&gpu_rcd));
        println!("  VNG4     CPU {:?} GPU {:?}", px(&cpu_vng), px(&gpu_vng));
        println!("  RCD dual CPU {:?} GPU {:?}", px(&cpu_dual), px(&gpu_dual));
        let apart: Vec<f32> = px(&cpu_rcd)
            .iter()
            .zip(px(&cpu_vng))
            .map(|(a, b)| (a - b).abs())
            .collect();
        println!("  RCD and VNG4 apart by {apart:?}");
    }
}

/// The GPU's demosaics against the CPU's, timed on synthetic frames of
/// 24 and 45 MP: the AMaZE dual (the default), the RCD dual, and RCD
/// and VNG4 alone, best of three warm on the GPU and once on the CPU
/// over every thread rayon has. A measurement, not a test.
#[test]
#[ignore]
fn timing() {
    let Some(ctx) = context() else {
        return;
    };
    println!(
        "on {}, CPU on {} threads",
        ctx.name(),
        rayon::current_num_threads()
    );
    let p = pattern("RGGB");
    let contrast = DualContrast::Noise(NoiseModel {
        a: [4e-4, 2e-4, 5e-4],
        b: [2e-6, 1e-6, 3e-6],
    });
    for (w, h) in [(6000usize, 4000usize), (8256, 5504)] {
        let m = mosaic(w, h, &p, 0.01, 3);
        // The automatic threshold's search reads the detail result on
        // the host, so it takes a path of its own: on a scene with the
        // flat tile it looks for.
        let flat = flat_with_an_edge(w, h, &p, 3);
        type Run<'a> = Box<dyn Fn() -> Vec<f32> + 'a>;
        let ops: [(&str, Run, Run); 6] = [
            (
                "AMaZE dual",
                Box::new(|| ctx.demosaic_amaze_dual(&m, w, h, &p, contrast).unwrap()),
                Box::new(|| {
                    demosaic_dual(&m, w, h, &p, DemosaicMethod::Amaze, contrast)
                        .unwrap()
                        .0
                }),
            ),
            (
                "RCD dual",
                Box::new(|| ctx.demosaic_rcd_dual(&m, w, h, &p, contrast).unwrap()),
                Box::new(|| {
                    demosaic_dual(&m, w, h, &p, DemosaicMethod::Rcd, contrast)
                        .unwrap()
                        .0
                }),
            ),
            (
                "AMaZE dual, automatic",
                Box::new(|| {
                    ctx.demosaic_amaze_dual(&flat, w, h, &p, DualContrast::Tiles)
                        .unwrap()
                }),
                Box::new(|| {
                    demosaic_dual(&flat, w, h, &p, DemosaicMethod::Amaze, DualContrast::Tiles)
                        .unwrap()
                        .0
                }),
            ),
            (
                "RCD dual, automatic",
                Box::new(|| {
                    ctx.demosaic_rcd_dual(&flat, w, h, &p, DualContrast::Tiles)
                        .unwrap()
                }),
                Box::new(|| {
                    demosaic_dual(&flat, w, h, &p, DemosaicMethod::Rcd, DualContrast::Tiles)
                        .unwrap()
                        .0
                }),
            ),
            (
                "RCD",
                Box::new(|| ctx.demosaic_rcd(&m, w, h, &p).unwrap()),
                Box::new(|| demosaic_rcd(&m, w, h, &p).unwrap()),
            ),
            (
                "VNG4",
                Box::new(|| ctx.demosaic_vng4(&m, w, h, &p).unwrap()),
                Box::new(|| demosaic_vng4(&m, w, h, &p).unwrap()),
            ),
        ];
        for (name, gpu, cpu) in ops {
            let cold = std::time::Instant::now();
            gpu();
            let cold = cold.elapsed();
            ctx.take_demosaic_traffic();
            let mut warm = std::time::Duration::MAX;
            for _ in 0..3 {
                let t = std::time::Instant::now();
                gpu();
                warm = warm.min(t.elapsed());
            }
            let (up, back) = ctx.take_demosaic_traffic();
            let t = std::time::Instant::now();
            cpu();
            let cpu_time = t.elapsed();
            println!(
                "{w}x{h} ({:.1} MP) {name}: GPU {:.1} ms warm (best of 3), {:.1} ms cold, \
                 {} MB up and {} MB back a run; CPU {:.1} ms",
                (w * h) as f64 / 1e6,
                warm.as_secs_f64() * 1e3,
                cold.as_secs_f64() * 1e3,
                (up / 3) >> 20,
                (back / 3) >> 20,
                cpu_time.as_secs_f64() * 1e3,
            );
        }
    }
}
