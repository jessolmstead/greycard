//! The render gate: existing edits, as a sidecar holds them, exported
//! on the CPU reference over synthetic frames and held to the pictures
//! kept of them. A change that moves what an existing edit renders to
//! fails here, so that it is a decision: the process bumped and the
//! kept pictures made again on purpose, or what moved put back.
//!
//! The edits are `tests/render-gate/edits/<case>.json`, each an edit as
//! a sidecar holds it, read back through `Edit::from_json` and its
//! migration, and the frame's shape step, as a sidecar's is: a field
//! renamed, a default changed, or a migration step edited reaches them
//! as it reaches the user's sidecars. Most are at the current schema;
//! the `migrate-*` ones are written as older schemas wrote them. The
//! kept pictures are `tests/render-gate/kept/<case>.png`, 8-bit sRGB as
//! an export's PNG holds it, and `kept/process` names the process they
//! are of.
//!
//! `GREYCARD_BLESS=1` writes the kept pictures again once [`PROCESS`]
//! is ahead of the kept set's; `GREYCARD_BLESS=unpinned` writes what
//! moved without a bump, for a change we choose not to pin. Either way
//! the run fails after writing, so a bless is never green.
//! `render_gate_spread` (ignored) measures how far the pictures move
//! with the thread count and on the GPU, for the tolerance.

use super::*;
use greycard_core::raw::{
    Calibration, CfaPattern, LevelPattern, Levels, Orientation, Samples, SensorLayout,
};
use greycard_render::export::{Format, Metadata, Pixels, Rendered, Settings, Sharpen, Space};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The process the kept pictures are of: the edit's
/// ([`greycard_edit::PROCESS`]), the rendering a state made now is
/// stamped with. A change that moves a kept picture bumps that number
/// (and the pictures' key with it, `greycard_edit::key::PROCESS_KEYED`)
/// and blesses the set under it; before 1.0 no older set is kept.
const PROCESS: u32 = greycard_edit::PROCESS;

/// Where a picture outside the tolerance fails the gate. Elsewhere it
/// is made and compared and the report printed, but the test passes,
/// until `render_gate_spread` has been run on that platform.
const ENFORCED: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));

/// How far a kept picture may move, in levels of 8 bits, and still be
/// the same: the most any sample may move, and the most the samples
/// may move on average. From measurement (`render_gate_spread`): on
/// Linux x86 the CPU reference is bit-equal at one to eight threads,
/// and its device stages on lavapipe, or the frames made in f32 rather
/// than f64, move a case at most one level and 0.0006 on average. A
/// level over that on any one sample, and thirty times that on average;
/// the smallest change the gate was shown to catch (a 5% grain) moves
/// the mean 0.09 while no sample moves past one level.
const MAX_LEVELS: u8 = 2;
const MEAN_LEVELS: f64 = 0.02;

/// The variable that writes the kept pictures again.
const BLESS: &str = "GREYCARD_BLESS";

/// The threads the cases are made on: plain threads, never the pool's,
/// as the engine's worker is. A walk waits on threads of its own whose
/// work is the pool's (the masks' detail planes), and a pool worker
/// waiting so, with every other worker waiting too, waits for ever.
const WORKERS: usize = 4;

/// The gate's folder, beside the crate's sources.
fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/render-gate")
}

/// The workspace's folder, two above the crate's.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits under crates/ in the workspace")
        .to_path_buf()
}

/// Where a run that moved writes what it made, what was kept, and
/// the difference: the build's target folder, the workspace's unless
/// the run names another (a relative one from the workspace, where
/// cargo is run).
fn out_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(|t| workspace().join(t))
        .unwrap_or_else(|| workspace().join("target"))
        .join("render-gate")
}

/// What a case's edit is exported over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sensor {
    /// The chart ([`chart`]) on a Bayer mosaic.
    Bayer,
    /// The chart on an X-Trans mosaic.
    XTrans,
    /// Two flat greys ([`patches`]) on a Bayer mosaic, a high ISO's
    /// noise on them: enough flat blocks at one level for the noise to
    /// be measured, which the chart's few are not, so the profiled
    /// denoise acts.
    Patches,
    /// A texture ([`texture`]) on a Bayer mosaic of 400 by 320, its
    /// red landed a little too far out and its blue a little too far
    /// in: the automatic lateral aberration's correction measures
    /// nothing on a frame under 256 pixels a side or with few tiles.
    Aberrated,
    /// A sky blown over a wide glow and a small sun ([`sky`]) on a
    /// Bayer mosaic of [`SKY`]: the learned highlight fill's regions,
    /// one in hl-2's size gate and one under it.
    Highlights,
}

/// The sky frame's size: its glow's clipped region is deep enough to
/// sit within hl-2's size gate, where its fill fades out, so hl-1 and
/// hl-2 differ there and agree on the sun (measured: at 720 by 540 the
/// glow is under the gate and the two are the same to the bit).
const SKY: (usize, usize) = (840, 630);

impl Sensor {
    /// The frame's size.
    fn size(self) -> (usize, usize) {
        match self {
            Sensor::Aberrated => (400, 320),
            Sensor::Highlights => SKY,
            _ => (W, H),
        }
    }
}

/// One kept picture: an edit by name, the frame it is exported from,
/// and what the export sheet asks beyond the edit.
struct Case {
    name: &'static str,
    sensor: Sensor,
    /// The sheet's long edge; none is the picture's own size.
    long_edge: Option<u32>,
    /// The sheet's Sharpen, which acts only after a resize.
    sharpen: Sharpen,
}

/// A case at full size from the Bayer frame, the sheet at its default.
const fn full(name: &'static str) -> Case {
    Case {
        name,
        sensor: Sensor::Bayer,
        long_edge: None,
        sharpen: Sharpen::Standard,
    }
}

/// A case at full size from the patches.
const fn flat(name: &'static str) -> Case {
    Case {
        sensor: Sensor::Patches,
        ..full(name)
    }
}

/// A case exported smaller, to a long edge of 64 from 96, with the
/// sheet's `sharpen`.
const fn smaller(name: &'static str, sharpen: Sharpen) -> Case {
    Case {
        long_edge: Some(64),
        sharpen,
        ..full(name)
    }
}

/// A case at full size from the X-Trans frame.
const fn xtrans(name: &'static str) -> Case {
    Case {
        sensor: Sensor::XTrans,
        ..full(name)
    }
}

/// A case from a larger frame, exported to the chart's long edge with
/// the sheet's Sharpen off, so the picture kept is as small as the
/// others.
const fn large(name: &'static str, sensor: Sensor) -> Case {
    Case {
        sensor,
        long_edge: Some(W as u32),
        sharpen: Sharpen::Off,
        ..full(name)
    }
}

/// Every case, in the order the report lists them. Each has its edit
/// in `edits/` and its picture in `kept/`; a file with no case here
/// fails the gate, so none is forgotten.
const CASES: &[Case] = &[
    full("default"),
    full("light"),
    full("whites-down"),
    full("agx"),
    full("white-balance"),
    full("color"),
    full("mixer"),
    full("grading"),
    full("tint"),
    full("curves"),
    full("black-and-white"),
    full("texture-clarity"),
    full("structure"),
    full("dehaze"),
    full("haze-in"),
    full("capture-sharpen"),
    full("capture-sharpen-off"),
    full("output-sharpen"),
    smaller("output-sharpen-smaller", Sharpen::Standard),
    flat("patches"),
    flat("noise"),
    full("lens"),
    full("lens-scale"),
    full("defringe"),
    full("crop-straighten"),
    full("keystone"),
    full("turn-flip"),
    full("vignette"),
    full("vignette-display"),
    full("grain"),
    full("grain-tabular"),
    full("mask-linear"),
    full("mask-radial"),
    full("mask-white-held"),
    full("mask-brush"),
    full("mask-luminance"),
    full("mask-color"),
    full("mask-joined"),
    full("mask-refine"),
    full("mask-detail"),
    smaller("mask-detail-smaller", Sharpen::Standard),
    // A mask's Output sharpen at a Radius and a Threshold of its own:
    // +200 at radius 3, a radius alone, and a take whose own radius
    // does nothing; smaller, the sheet's Sharpen giving way to it.
    full("mask-output-sharpen-own"),
    smaller("mask-output-sharpen-own-smaller", Sharpen::Standard),
    full("retouch"),
    smaller("smaller", Sharpen::Off),
    smaller("smaller-sheet-sharpen", Sharpen::Standard),
    full("demosaic-rcd-vng4"),
    full("demosaic-amaze"),
    full("demosaic-rcd"),
    full("demosaic-vng4"),
    full("demosaic-bilinear"),
    xtrans("xtrans"),
    xtrans("xtrans-markesteijn-1"),
    large("chromatic-aberration", Sensor::Aberrated),
    large("highlights", Sensor::Highlights),
    large("highlight-fill-hl-1", Sensor::Highlights),
    large("highlight-fill-hl-2", Sensor::Highlights),
    // Written as older schemas wrote them, so each migration step that
    // keeps a look runs: version 1's denoise switch, version 4's crop
    // on a turned frame (its step that waits on the frame's shape),
    // version 6's baseline (greycard-edit's own sidecar fixture from
    // before the versions, its `current`), and version 8's brushes.
    flat("migrate-v1-denoise"),
    full("migrate-v3-original-aspect"),
    full("migrate-v4-baseline"),
    full("migrate-v7-brush"),
];

/// The chart's size: small enough that the gate is quick in a debug
/// build, large enough for every op's window.
const W: usize = 96;
const H: usize = 64;

/// splitmix64 of a pixel's index, as a triangle from -1 to 1 (two
/// uniforms summed, near enough a normal's shape for a denoise to
/// read): the frames' noise, the same on every machine.
fn noise(i: u64) -> f64 {
    let mut z = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let a = (z >> 40) as f64 / (1u64 << 24) as f64;
    let b = ((z >> 8) & 0xFF_FFFF) as f64 / (1u64 << 24) as f64;
    a + b - 1.0
}

/// The patches at a point: mid grey over the left three quarters,
/// enough blocks at one level for the noise estimate, and a light grey
/// beside.
fn patches(x: f64, _y: f64) -> [f64; 3] {
    [if x < (W * 3 / 4) as f64 { 0.18 } else { 0.5 }; 3]
}

/// A texture of crossing waves at every scale, in the frame's own
/// pixels: edges in every tile for the aberration's measurement, as
/// the worker's own aberrated frame has them.
fn texture(x: f64, y: f64) -> [f64; 3] {
    let v = (x * 0.11).sin() * (y * 0.07).cos()
        + (x * 0.031 + y * 0.052).sin()
        + ((x * 0.9 + y * 0.37) * 0.23).sin() * 0.5
        + ((x - y) * 0.017).cos() * 0.7;
    [0.05 + 0.4 * (0.5 + v / 6.4); 3]
}

/// The chart at a point, in pixels of a frame `W` by `H`, linear,
/// white at one: a band of hues over lightness at the top; a grey ramp
/// from seven stops under mid grey to three over it, where green
/// clips alone, with a warm strip under it, in the middle; and at the
/// bottom stripes rising to the mosaic's Nyquist, a checker round a
/// disc past every channel's clip, and a soft glow through green's
/// clip.
fn chart(x: f64, y: f64) -> [f64; 3] {
    let fx = x / W as f64;
    if y < 20.0 {
        let hue = fx * 6.0;
        let lightness = 0.04 + 0.5 * y / 20.0;
        let channel = |offset: f64| {
            let d = ((hue - offset).rem_euclid(6.0) - 3.0).abs();
            (d - 1.0).clamp(0.0, 1.0)
        };
        let chroma = 0.85;
        return [channel(0.0), channel(4.0), channel(2.0)]
            .map(|c| lightness * (1.0 - chroma + chroma * c));
    }
    if y < 40.0 {
        let v = 0.18 * (-7.0 + 10.0 * fx).exp2();
        return if y < 33.0 {
            [v; 3]
        } else {
            [v * 1.25, v * 0.95, v * 0.7]
        };
    }
    let fy = y - 40.0;
    if x < 32.0 {
        let s = (x * x * 0.05 + fy * 0.6).sin();
        return [0.2 + 0.18 * s; 3];
    }
    if x < 64.0 {
        let (cx, cy) = (x - 48.0, fy - 12.0);
        if cx * cx + cy * cy < 25.0 {
            return [3.0; 3];
        }
        return if ((x / 6.0).floor() + (y / 6.0).floor()) % 2.0 == 0.0 {
            [0.9, 0.85, 0.8]
        } else {
            [0.01, 0.012, 0.015]
        };
    }
    // Green from half its clip at the glow's rim to 1.2 times it at
    // its middle; red and blue stay under theirs.
    let r2 = ((x - 80.0).powi(2) + (fy - 12.0).powi(2)) / (18.0 * 18.0);
    let v = 0.5 + 0.7 * (1.0 - r2).max(0.0);
    [0.9 * v, v, 0.85 * v]
}

/// The sky at a point, in pixels of a frame [`SKY`]: a blue sky over a
/// dark textured ground, a wide glow that clips green and blue over a
/// region within hl-2's size gate and red over a smaller core, and a
/// small sun clipped through.
fn sky(x: f64, y: f64) -> [f64; 3] {
    let (w, h) = (SKY.0 as f64, SKY.1 as f64);
    if y > 0.85 * h {
        let t = texture(x, y)[0];
        return [0.5 * t, 0.45 * t, 0.35 * t];
    }
    let base = [0.18, 0.3, 0.55].map(|c| c * (1.2 - 0.4 * y / h));
    let glow = {
        let r2 = ((x - 0.35 * w).powi(2) + (y - 0.42 * h).powi(2)) / (0.5 * h).powi(2);
        2.6 * (1.0 - r2).max(0.0)
    };
    let sun = {
        let r2 = ((x - 0.92 * w).powi(2) + (y - 0.2 * h).powi(2)) / (0.04 * h).powi(2);
        4.0 * (1.0 - r2).max(0.0).sqrt()
    };
    let warm = [1.0, 0.95, 0.85];
    [0, 1, 2].map(|c| base[c] + (glow + sun) * warm[c])
}

/// The as-shot coefficients the frames are made under, green at one,
/// and their levels: fourteen bits over a black of 512.
const AS_SHOT: [f64; 3] = [2.0, 1.0, 1.6];
const BLACK: f64 = 512.0;
const WHITE: f64 = 16383.0;

/// A scene: its linear RGB at a point of the frame.
type Scene = fn(f64, f64) -> [f64; 3];

/// Where in a pixel the scene is sampled, sixteen times, over a box a
/// pixel wide: the blur of the lens and the filter stack, which every
/// real raw has. A camera makes no edge as hard as one pixel, and at
/// one beside clipped light Markesteijn's choice of direction ties,
/// every derivative on the plateau zero, so an ulp anywhere upstream
/// moved the X-Trans case 25 levels.
const SPREAD: [f64; 4] = [-0.5625, -0.1875, 0.1875, 0.5625];

/// The gate's own sensor: the matrices of an ordinary one, a little
/// wider than Rec.2020 in its reds and a little narrower in its blues,
/// under a warm and a daylight light, so a white balance by
/// temperature resolves. Its own, not a shared test helper's, so no
/// other test's need moves the kept pictures.
fn calibrations() -> Vec<Calibration> {
    let calibration = |illuminant: u16, m: [f32; 9]| Calibration {
        illuminant,
        color_matrix: m.to_vec(),
        forward_matrix: None,
    };
    vec![
        calibration(
            17,
            [
                1.1564, -0.4626, -0.1081, -0.4229, 1.2260, 0.2187, -0.0711, 0.1500, 0.6268,
            ],
        ),
        calibration(
            21,
            [
                0.9766, -0.2953, -0.1254, -0.4276, 1.2116, 0.2433, -0.0437, 0.1336, 0.5131,
            ],
        ),
    ]
}

/// Hot photosites on the patches' frame, `(x, y)`, at four times what
/// the scene puts there: reds, greens, and blues on the mid grey, for
/// the hot pixel repair, which a clean frame leaves with nothing to do.
const HOT: [(usize, usize); 5] = [(10, 10), (41, 20), (31, 30), (25, 41), (60, 52)];

/// A raw frame of the scene on `sensor`, with shot and read noise from
/// a seeded hash. Made in code every run: no raw, and no slice of one,
/// is in the tree. The patches are a high ISO's, eight times noisier,
/// so the denoise's work shows in 8 bits, and carry a few hot
/// photosites ([`HOT`]).
fn frame(sensor: Sensor) -> RawFrame {
    let (pattern, scene, iso): (_, Scene, f64) = match sensor {
        Sensor::Bayer => (CfaPattern::rggb(), chart, 1.0),
        Sensor::XTrans => (CfaPattern::xtrans(), chart, 1.0),
        Sensor::Patches => (CfaPattern::rggb(), patches, 8.0),
        Sensor::Aberrated => (CfaPattern::rggb(), texture, 1.0),
        Sensor::Highlights => (CfaPattern::rggb(), sky, 1.0),
    };
    let (w, h) = sensor.size();
    // Each color's magnification about the center, on the aberrated
    // frame.
    let (cx, cy) = ((w - 1) as f64 / 2.0, (h - 1) as f64 / 2.0);
    let magnified = match sensor {
        Sensor::Aberrated => [1.005, 1.0, 0.996],
        _ => [1.0; 3],
    };
    let range = WHITE - BLACK;
    let mut samples = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let c = pattern.color_at(y, x).rgb_index().expect("an RGB pattern");
            let m = magnified[c];
            let (u, v) = (cx + (x as f64 - cx) / m, cy + (y as f64 - cy) / m);
            let light = SPREAD
                .iter()
                .flat_map(|dy| SPREAD.iter().map(move |dx| scene(u + dx, v + dy)[c]))
                .sum::<f64>()
                / 16.0;
            let hot = sensor == Sensor::Patches && HOT.contains(&(x, y));
            let signal = light / AS_SHOT[c] * range * if hot { 4.0 } else { 1.0 };
            let sd = iso * (signal * 1.5 + 400.0).sqrt();
            let v = BLACK + signal + sd * noise((y * w + x) as u64);
            samples.push(v.round().clamp(0.0, WHITE) as u16);
        }
    }
    RawFrame {
        make: "Test".into(),
        model: "Cam".into(),
        other_names: Vec::new(),
        width: w,
        height: h,
        channels: 1,
        layout: SensorLayout::Cfa(pattern),
        samples: Samples::U16(samples),
        levels: Levels {
            black: LevelPattern::uniform(BLACK as f32, 1),
            white: LevelPattern::uniform(WHITE as f32, 1),
            black_deltas: None,
            saturation: None,
        },
        as_shot_coefficients: Some(AS_SHOT),
        calibrations: calibrations(),
        crop: None,
        orientation: Orientation::Normal,
        shot: Default::default(),
        dng: Default::default(),
    }
}

/// A case's edit, read as a sidecar's is: migrated, and its version 4
/// step finished with the frame's shape, as `greycard_edit_io` does
/// when it opens a sidecar beside a frame.
fn edit(case: &Case) -> Edit {
    let path = dir().join("edits").join(format!("{}.json", case.name));
    let json = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut edit = Edit::from_json(&json).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (w, h) = case.sensor.size();
    edit.migrate_with_frame(h > w);
    edit
}

/// `edit` exported as `case` asks, on the CPU unless `gpu` lends a
/// device: the export's own walk and finish (`render_open`), the
/// pixels its PNG holds. Never on a pool worker ([`WORKERS`]).
fn render(case: &Case, edit: &Edit, gpu: &mut Option<greycard_gpu::Context>) -> Picture8 {
    assert!(
        rayon::current_thread_index().is_none(),
        "{}: an export walks on a thread of its own, not the pool's, which its detail planes need",
        case.name
    );
    let settings = Settings {
        format: Format::Png,
        long_edge: case.long_edge,
        space: Space::Srgb,
        embed_profile: false,
        sharpen: case.sharpen,
        metadata: Metadata::None,
        ..Settings::default()
    };
    let input = Input::Raw(Arc::new(frame(case.sensor)));
    let deliver: Deliver = Arc::new(|_| {});
    let mut ai = Ai::new();
    let (rendered, left_out) = super::export::render_open(
        edit,
        0,
        &input,
        None,
        &mut WorkerModels::new(&mut ai, None, &deliver, 0),
        None,
        gpu,
        &settings,
    )
    .unwrap_or_else(|e| panic!("{}: not exported: {e}", case.name));
    assert!(
        left_out.is_empty(),
        "{}: exported without {left_out:?}",
        case.name
    );
    Picture8::of(rendered)
}

/// Each of `cases` made on the CPU, in their order, on [`WORKERS`]
/// plain threads taking them in turn.
fn render_all(cases: &[&Case]) -> Vec<Picture8> {
    let next = AtomicUsize::new(0);
    let made: Mutex<Vec<Option<Picture8>>> = Mutex::new(vec![None; cases.len()]);
    std::thread::scope(|s| {
        for _ in 0..WORKERS {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(case) = cases.get(i) else {
                        break;
                    };
                    let picture = render(case, &edit(case), &mut None);
                    made.lock().expect("the pictures")[i] = Some(picture);
                }
            });
        }
    });
    made.into_inner()
        .expect("the pictures")
        .into_iter()
        .map(|p| p.expect("every case made"))
        .collect()
}

/// An 8-bit RGB picture, as made or as kept.
#[derive(Clone, PartialEq)]
struct Picture8 {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl Picture8 {
    fn of(r: Rendered) -> Self {
        match r.pixels {
            Pixels::Eight(rgb) => Self {
                width: r.width,
                height: r.height,
                rgb,
            },
            Pixels::Sixteen(_) => unreachable!("a PNG of eight bits"),
        }
    }

    fn read(path: &Path) -> Option<Self> {
        let image = image::open(path).ok()?.into_rgb8();
        Some(Self {
            width: image.width(),
            height: image.height(),
            rgb: image.into_raw(),
        })
    }

    /// Written as a PNG, compressed as tightly as the encoder goes: a
    /// kept picture is in the tree.
    fn write(&self, path: &Path) {
        use image::ImageEncoder;
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};
        let mut bytes = Vec::new();
        PngEncoder::new_with_quality(&mut bytes, CompressionType::Best, FilterType::Adaptive)
            .write_image(
                &self.rgb,
                self.width,
                self.height,
                image::ExtendedColorType::Rgb8,
            )
            .expect("the size is the pixels'");
        std::fs::write(path, bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    /// How far `self` is from `kept`; none when their sizes differ.
    fn moved_from(&self, kept: &Picture8) -> Option<Moved> {
        if (self.width, self.height) != (kept.width, kept.height) {
            return None;
        }
        let (mut max, mut sum, mut over) = (0u8, 0u64, 0usize);
        for (a, b) in self.rgb.iter().zip(&kept.rgb) {
            let d = a.abs_diff(*b);
            max = max.max(d);
            sum += u64::from(d);
            if d > 1 {
                over += 1;
            }
        }
        Some(Moved {
            max,
            mean: sum as f64 / self.rgb.len() as f64,
            over,
            samples: self.rgb.len(),
        })
    }

    /// The difference from `kept`, sixteen times over, so a level
    /// shows.
    fn difference(&self, kept: &Picture8) -> Picture8 {
        Picture8 {
            rgb: self
                .rgb
                .iter()
                .zip(&kept.rgb)
                .map(|(a, b)| a.abs_diff(*b).saturating_mul(16))
                .collect(),
            ..self.clone()
        }
    }
}

/// How far a picture moved from the kept one, in levels of 8 bits.
#[derive(Clone, Copy, Debug)]
struct Moved {
    max: u8,
    mean: f64,
    /// How many samples moved more than one level.
    over: usize,
    samples: usize,
}

impl Moved {
    fn within(&self) -> bool {
        self.max <= MAX_LEVELS && self.mean <= MEAN_LEVELS
    }
}

impl std::fmt::Display for Moved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "max {} levels, mean {:.4}, {} of {} samples over one level",
            self.max, self.mean, self.over, self.samples
        )
    }
}

/// What a case came to against its kept picture.
enum Verdict {
    Same,
    Moved(Moved),
    Resized { made: (u32, u32), kept: (u32, u32) },
    Missing,
}

impl Verdict {
    fn of(name: &str, made: &Picture8) -> Self {
        match Picture8::read(&dir().join("kept").join(format!("{name}.png"))) {
            None => Verdict::Missing,
            Some(kept) => match made.moved_from(&kept) {
                None => Verdict::Resized {
                    made: (made.width, made.height),
                    kept: (kept.width, kept.height),
                },
                Some(m) if m.within() => Verdict::Same,
                Some(m) => Verdict::Moved(m),
            },
        }
    }

    /// The case's line in a report; none when it is the same.
    fn line(&self, name: &str) -> Option<String> {
        match self {
            Verdict::Same => None,
            Verdict::Missing => Some(format!("{name}: no kept picture")),
            Verdict::Resized { made, kept } => Some(format!(
                "{name}: made {}x{}, kept {}x{}",
                made.0, made.1, kept.0, kept.1
            )),
            Verdict::Moved(m) => Some(format!("{name}: moved {m}")),
        }
    }
}

/// The kept set's process, from `kept/process`; none without one.
fn kept_process() -> Option<u32> {
    std::fs::read_to_string(dir().join("kept/process"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The names of the files in `folder` ending in `ext`, sorted.
fn names(folder: &str, ext: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir().join(folder))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str()?.strip_suffix(ext).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// What `GREYCARD_BLESS` asks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bless {
    Off,
    /// `1`: the set under a process bumped ahead of the kept one, and
    /// pictures for new cases.
    Pinned,
    /// `unpinned`: what moved, under the same process, for a change we
    /// choose not to pin (a decode or a bug fix).
    Unpinned,
}

impl Bless {
    fn asked() -> Self {
        match std::env::var(BLESS).as_deref() {
            Err(_) | Ok("") => Bless::Off,
            Ok("1") => Bless::Pinned,
            Ok("unpinned") => Bless::Unpinned,
            Ok(other) => panic!("{BLESS} is 1 or unpinned, not {other:?}"),
        }
    }
}

/// How to bless on purpose, for every message that says the gate
/// moved.
fn how_to_bless() -> String {
    format!(
        "A change that moves an existing edit's picture is a decision, not an accident. \
         Put back what moved, or move the look on purpose: bump the process (PROCESS in \
         crates/greycard-edit/src/lib.rs, with the pictures' key as its test says), write \
         the changelog line that says what moves, and make the kept pictures again with\n  \
         {BLESS}=1 cargo test -p greycard-engine --lib render_gate\n\
         For a change we choose not to pin (a decode or a bug fix), say so in its notes \
         section and bless what moved under the same process with\n  \
         {BLESS}=unpinned cargo test -p greycard-engine --lib render_gate\n\
         A bless writes the pictures and then fails; run again without {BLESS}, and commit \
         the pictures with the change."
    )
}

/// Every existing edit renders as it was kept: within the tolerance
/// of its picture in `kept/`, on the CPU reference an export takes.
/// Under `GREYCARD_BLESS` the pictures are written instead, and the
/// run fails saying what it wrote.
#[test]
fn existing_edits_render_as_kept() {
    let bless = Bless::asked();
    assert!(
        bless == Bless::Off || std::env::var_os("CI").is_none_or(|v| v.is_empty()),
        "{BLESS} is refused under CI: kept pictures are made on purpose, on a machine, and committed"
    );
    let kept_dir = dir().join("kept");
    let kept = kept_process();
    if bless != Bless::Off {
        // The pinned bless reads "a new process" off this file, so a
        // missing or broken one must not pass for one: missing only
        // before the first bless, when no picture is kept.
        let held = names("kept", ".png").len();
        match std::fs::read_to_string(kept_dir.join("process")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => assert!(
                held == 0,
                "{BLESS} is refused: kept/process is missing beside {held} kept pictures; \
                 restore it before a bless"
            ),
            Err(e) => panic!("{BLESS} is refused: kept/process cannot be read: {e}"),
            Ok(text) => assert!(
                matches!(text.trim().parse::<u32>(), Ok(p) if (1..=PROCESS).contains(&p)),
                "{BLESS} is refused: kept/process holds {:?}, not a process from 1 to {PROCESS}",
                text.trim()
            ),
        }
    }
    // Each case has an edit, and each edit and kept picture a case.
    let cases: Vec<&str> = CASES.iter().map(|c| c.name).collect();
    let mut sorted = cases.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), cases.len(), "a case is named twice");
    assert_eq!(
        names("edits", ".json"),
        sorted,
        "every edit in {} has a case in CASES, and every case an edit",
        dir().join("edits").display()
    );
    let stray: Vec<String> = names("kept", ".png")
        .into_iter()
        .filter(|n| !cases.contains(&n.as_str()))
        .collect();

    let made = render_all(&CASES.iter().collect::<Vec<_>>());
    let verdicts: Vec<Verdict> = CASES
        .iter()
        .zip(&made)
        .map(|(case, picture)| Verdict::of(case.name, picture))
        .collect();

    if bless != Bless::Off {
        // A new process (or the first bless) makes the whole set;
        // under the same one only what is new, or what moved when
        // unpinned is said.
        let renewed = kept != Some(PROCESS);
        let moved: Vec<String> = CASES
            .iter()
            .zip(&verdicts)
            .filter(|(_, v)| matches!(v, Verdict::Moved(_) | Verdict::Resized { .. }))
            .filter_map(|(c, v)| v.line(c.name))
            .collect();
        assert!(
            renewed || bless == Bless::Unpinned || moved.is_empty(),
            "{BLESS}=1 rewrites moved pictures only under a new process, and process {PROCESS} \
             is the kept set's. What moved:\n  {}\nBump PROCESS, or bless with \
             {BLESS}=unpinned for a change we choose not to pin.",
            moved.join("\n  ")
        );
        std::fs::create_dir_all(&kept_dir).unwrap();
        let mut written = Vec::new();
        for ((case, picture), verdict) in CASES.iter().zip(&made).zip(&verdicts) {
            if renewed || !matches!(verdict, Verdict::Same) {
                picture.write(&kept_dir.join(format!("{}.png", case.name)));
                written.push(format!(
                    "kept/{}.png: {}",
                    case.name,
                    match verdict {
                        Verdict::Missing => "new".to_string(),
                        Verdict::Same => "within the tolerance, the set renewed".to_string(),
                        v => v.line(case.name).unwrap_or_default(),
                    }
                ));
            }
        }
        for name in &stray {
            std::fs::remove_file(kept_dir.join(format!("{name}.png"))).unwrap();
            written.push(format!("kept/{name}.png: removed, no case"));
        }
        std::fs::write(kept_dir.join("process"), format!("{PROCESS}\n")).unwrap();
        panic!(
            "render gate: blessed{} for process {PROCESS} ({} of {} pictures written, {} \
             removed):\n  {}\nRun again without {BLESS}; a bless never passes.",
            if bless == Bless::Unpinned {
                ", unpinned,"
            } else {
                ""
            },
            written.len() - stray.len(),
            made.len(),
            stray.len(),
            if written.is_empty() {
                "nothing".to_string()
            } else {
                written.join("\n  ")
            }
        );
    }

    let mut lines = Vec::new();
    if kept != Some(PROCESS) {
        lines.push(format!(
            "the kept pictures are of process {}, and the gate is at process {PROCESS}: \
             bless the set under the new process",
            kept.map_or("none".into(), |p| p.to_string())
        ));
    }
    for name in &stray {
        lines.push(format!("{name}: a kept picture with no case"));
    }
    let out = out_dir();
    for ((case, picture), verdict) in CASES.iter().zip(&made).zip(&verdicts) {
        let Some(line) = verdict.line(case.name) else {
            continue;
        };
        lines.push(line);
        std::fs::create_dir_all(&out).unwrap();
        picture.write(&out.join(format!("{}.got.png", case.name)));
        if let Some(kept) = Picture8::read(&kept_dir.join(format!("{}.png", case.name))) {
            kept.write(&out.join(format!("{}.kept.png", case.name)));
            if picture.moved_from(&kept).is_some() {
                picture
                    .difference(&kept)
                    .write(&out.join(format!("{}.diff.png", case.name)));
            }
        }
    }
    if lines.is_empty() {
        return;
    }
    let report = format!(
        "render gate, process {PROCESS}: what an existing edit renders to has moved \
         (tolerance: max {MAX_LEVELS} levels, mean {MEAN_LEVELS}):\n  {}\n\
         Made, kept, and difference (x16) pictures are in {}.\n{}",
        lines.join("\n  "),
        out.display(),
        how_to_bless()
    );
    if ENFORCED {
        panic!("{report}");
    }
    eprintln!("not enforced on this platform until measured on it:\n{report}");
}

/// `cases` made in this process and compared with their kept pictures,
/// each that moved at all and the worst printed under `config`, and
/// those outside the tolerance returned: what the children below run,
/// in a process of their own, under a rayon thread count of their
/// choosing.
fn compare(config: &str, cases: &[&Case], made: &[Picture8]) -> Vec<String> {
    let mut outside = Vec::new();
    let mut worst: Option<(&str, Moved)> = None;
    let mut most_mean = 0.0f64;
    for (case, picture) in cases.iter().zip(made) {
        let kept = Picture8::read(&dir().join("kept").join(format!("{}.png", case.name)))
            .unwrap_or_else(|| panic!("{}: no kept picture", case.name));
        let moved = picture
            .moved_from(&kept)
            .unwrap_or_else(|| panic!("{}: another size", case.name));
        if moved.max > 0 {
            println!("{config:>12} {:<28} {moved}", case.name);
        }
        if !moved.within() {
            outside.push(format!("{config}: {}: {moved}", case.name));
        }
        if worst.is_none_or(|(_, w)| (moved.max, moved.mean) > (w.max, w.mean)) {
            worst = Some((case.name, moved));
        }
        most_mean = most_mean.max(moved.mean);
    }
    if let Some((name, w)) = worst {
        println!("{config:>12}: worst {w} ({name}), the highest mean {most_mean:.4}");
    }
    outside
}

/// `cases` made at this process's rayon thread count and compared,
/// failing outside the tolerance whatever the platform.
fn child_run(cases: &[&Case]) {
    // Off the line the test harness began with the test's name.
    println!();
    let config = format!("{} threads", rayon::current_num_threads());
    let made = render_all(cases);
    let outside = compare(&config, cases, &made);
    assert!(
        outside.is_empty(),
        "outside the tolerance (max {MAX_LEVELS} levels, mean {MEAN_LEVELS}):\n  {}",
        outside.join("\n  ")
    );
}

/// Every case at this process's rayon thread count: the spread's child.
#[test]
#[ignore = "run in a process of its own by render_gate_spread"]
fn render_gate_child() {
    child_run(&CASES.iter().collect::<Vec<_>>());
}

/// The cases on the chart's and the patches' small frames at this
/// process's rayon thread count: the one-thread check's child, the
/// large frames left out for its time (a minute at one thread in a
/// debug build, against seven seconds).
#[test]
#[ignore = "run in a process of its own by the one-thread check"]
fn render_gate_child_small() {
    child_run(
        &CASES
            .iter()
            .filter(|c| c.sensor.size() == (W, H))
            .collect::<Vec<_>>(),
    );
}

/// This test binary's `test` in a process with `threads` rayon threads,
/// waited for `limit`: its output, and whether it passed; none when it
/// did not finish, killed. Its output goes to a file under the target
/// folder, so a full pipe cannot stop it.
fn child(test: &str, threads: usize, limit: std::time::Duration) -> Option<(String, bool)> {
    let out = out_dir();
    std::fs::create_dir_all(&out).unwrap();
    let log = out.join(format!("{test}-{threads}.log"));
    let file = std::fs::File::create(&log).unwrap();
    let mut run = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args([
            &format!("worker::render_gate::{test}"),
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("RAYON_NUM_THREADS", threads.to_string())
        .env_remove(BLESS)
        .stdout(file.try_clone().unwrap())
        .stderr(file)
        .spawn()
        .expect("the test binary runs");
    let started = Instant::now();
    let status = loop {
        if let Some(status) = run.try_wait().expect("the child") {
            break status;
        }
        if started.elapsed() > limit {
            let _ = run.kill();
            let _ = run.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    Some((text, status.success()))
}

/// Whether a child's log shows its one test run to a verdict: a filter
/// that matches nothing (a renamed module) runs no test and exits 0.
fn ran_one(log: &str) -> bool {
    log.lines().any(|l| l.trim() == "running 1 test")
        && log.lines().any(|l| l.starts_with("test result: "))
}

/// The gate finishes with one rayon thread, as a one-core runner has
/// it: no case's render waits on work only the pool can do from a
/// thread the pool needs. The child must have run its test and, where
/// the gate is enforced, passed it.
#[test]
fn the_gate_finishes_at_one_rayon_thread() {
    let limit = std::time::Duration::from_secs(600);
    let Some((log, passed)) = child("render_gate_child_small", 1, limit) else {
        panic!(
            "the render gate did not finish in {} s with one rayon thread: a render waits on \
             work that needs the pool",
            limit.as_secs()
        );
    };
    assert!(ran_one(&log), "the one-thread child ran no test:\n{log}");
    assert!(passed || !ENFORCED, "the one-thread child failed:\n{log}");
}

/// How far the kept pictures move on this machine with nothing in the
/// edit or the code changed, and that it is within the tolerance: the
/// gate in processes of one, two, three, four, and eight rayon threads,
/// and with the export's device stages on the GPU when one is lent
/// (lavapipe here, `VK_DRIVER_FILES`). What the tolerance was set
/// from; run it on each platform CI builds for:
///   cargo test -p greycard-engine --lib render_gate_spread -- --ignored --nocapture
#[test]
#[ignore = "a measurement for the tolerance"]
fn render_gate_spread() {
    let mut outside = Vec::new();
    for threads in [1, 2, 3, 4, 8] {
        let config = format!("{threads} threads");
        match child(
            "render_gate_child",
            threads,
            std::time::Duration::from_secs(3600),
        ) {
            None => outside.push(format!("{config}: did not finish")),
            Some((text, passed)) => {
                let ours = |l: &&str| l.trim_start().starts_with(&config);
                for line in text.lines().filter(ours) {
                    println!("{line}");
                }
                if !ran_one(&text) {
                    outside.push(format!("{config}: the child ran no test:\n{text}"));
                } else if !passed {
                    outside.push(format!("{config}: the child failed:\n{text}"));
                }
            }
        }
    }
    let cases: Vec<&Case> = CASES.iter().collect();
    match greycard_gpu::Context::own() {
        Ok(ctx) => {
            let mut gpu = Some(ctx);
            let made: Vec<Picture8> = cases
                .iter()
                .map(|case| render(case, &edit(case), &mut gpu))
                .collect();
            outside.extend(compare("gpu", &cases, &made));
        }
        Err(_) => super::tests::skipped("the render gate's GPU spread"),
    }
    assert!(
        outside.is_empty(),
        "outside the tolerance (max {MAX_LEVELS} levels, mean {MEAN_LEVELS}):\n  {}",
        outside.join("\n  ")
    );
}
