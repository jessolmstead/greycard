//! What the learned highlight fill's stage costs a walk to the base on a
//! 24 MP synthetic Bayer frame: its seconds on every thread and on one,
//! and the most the walk holds at once over the same walk with the fill
//! off, counted by the allocator. A binary of its own, since the count
//! is the whole process's. A measurement, printed:
//! `cargo test -p greycard-engine --release --test highlight_fill_cost -- --ignored --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use greycard_core::raw::{
    Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Orientation, RawFrame, Samples,
    SensorLayout,
};
use greycard_edit::Edit;
use greycard_engine::stages::{Purpose, stages};
use greycard_engine::walk::{Input, Kept, NoHold, NoModels, Walked, plan, walk};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    let now = LIVE.fetch_add(by, Ordering::SeqCst) + by;
    PEAK.fetch_max(now, Ordering::SeqCst);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract, passed on.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                grew(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
            }
        }
        p
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// A Bayer raw `w` by `h`: a ramp whose right quarter clips on green, a
/// disk that clips on green and red, a grid of small lamps, and a fine
/// checker.
fn frame(w: usize, h: usize) -> Arc<RawFrame> {
    let pattern = CfaPattern::rggb();
    let scene = |x: f32, y: f32| {
        let ramp = 200.0 + 2400.0 * x / w as f32;
        let (dx, dy) = (x - w as f32 * 0.6, y - h as f32 * 0.4);
        let r = (h as f32 / 6.0).powi(2);
        let disk = if dx * dx + dy * dy < r { 1200.0 } else { 0.0 };
        let (lx, ly) = ((x % 300.0) - 150.0, (y % 300.0) - 150.0);
        let lamp = if x < w as f32 * 0.5 && lx * lx + ly * ly < 100.0 {
            3000.0
        } else {
            0.0
        };
        let checker = if ((x / 3.0).floor() + (y / 3.0).floor()) as i64 % 2 == 0 {
            150.0
        } else {
            0.0
        };
        ramp + disk + lamp + checker
    };
    let mut samples = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let gain = match pattern.color_at(y, x) {
                CfaColor::Red => 0.55,
                CfaColor::Green => 1.0,
                CfaColor::Blue => 0.4,
                CfaColor::Other(_) => unreachable!(),
            };
            samples.push((scene(x as f32, y as f32) * gain).min(4000.0) as u16);
        }
    }
    Arc::new(RawFrame {
        make: "Test".into(),
        model: "Cam".into(),
        other_names: Vec::new(),
        width: w,
        height: h,
        channels: 1,
        layout: SensorLayout::Cfa(pattern),
        samples: Samples::U16(samples),
        levels: Levels {
            black: LevelPattern::uniform(64.0, 1),
            white: LevelPattern::uniform(2000.0, 1),
            black_deltas: None,
            saturation: None,
        },
        as_shot_coefficients: Some([2.0, 1.0, 1.6]),
        calibrations: vec![Calibration {
            illuminant: 21,
            color_matrix: vec![
                0.9766, -0.2953, -0.1254, -0.4276, 1.2116, 0.2433, -0.0437, 0.1336, 0.5131,
            ],
            forward_matrix: None,
        }],
        crop: None,
        orientation: Orientation::Normal,
        shot: Default::default(),
        dng: Default::default(),
    })
}

/// The walk to the base, its peak over what was live before it.
fn to_base(edit: &Edit, input: &Input) -> (Walked, usize) {
    let steps = stages(edit, &Purpose::Prefetch, &input.facts(0), None);
    let plan = plan(&steps, &Purpose::Prefetch, false);
    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let mut kept = Kept::new();
    let walked = walk(
        &steps,
        &plan,
        input,
        &mut kept,
        &mut NoModels,
        &mut None,
        &NoHold,
    )
    .expect("walks");
    (walked, PEAK.load(Ordering::SeqCst) - before)
}

fn seconds(walked: &Walked, stage: &str) -> f64 {
    walked
        .seconds
        .iter()
        .filter(|(s, _)| *s == stage)
        .map(|(_, s)| s)
        .sum()
}

#[test]
#[ignore = "a measurement, printed: run in release"]
fn the_fill_at_24_megapixels() {
    let input = Input::Raw(frame(6000, 4000));
    measure(&input);
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(|| measure(&input));
}

fn measure(input: &Input) {
    let on = Edit {
        highlight_fill: Some("hl-1".into()),
        ..Edit::default()
    };
    let mb = |b: usize| b as f64 / 1_048_576.0;
    let (off, off_peak) = to_base(&Edit::default(), input);
    drop(off);
    let (walked, on_peak) = to_base(&on, input);
    let filled = walked.filled.clone().expect("the fill ran");
    let total: f64 = walked.seconds.iter().map(|(_, s)| s).sum();
    eprintln!(
        "{} threads: the fill {:.3} s of {total:.3} s to the base, {} pixels changed, \
         range shift {:?}; peak {:.0} MB on, {:.0} MB off, +{:.0} MB",
        rayon::current_num_threads(),
        seconds(&walked, "highlight fill"),
        filled.changed,
        filled.range_shift,
        mb(on_peak),
        mb(off_peak),
        mb(on_peak) - mb(off_peak),
    );
}
