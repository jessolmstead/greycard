//! What the highlight fill's own round holds at once (notes §719 §3),
//! the walk's part alone: the window's first round keeps the camera
//! picture before the fill, and the round after walks from it, its walk
//! holding its kept points of the picture without the fill only until
//! the fill is made. The frame's own references to that picture (its
//! base, bands, and guide) are the worker's to let go, measured through
//! the worker's develop in `worker::highlight_rounds`. Counted by the
//! allocator; a binary of its own, since the count is the whole
//! process's.
//!
//! The 24 MP figures, printed:
//! `cargo test -p greycard-engine --release --test highlight_fill_round -- --ignored --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use greycard_core::raw::{
    Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Orientation, RawFrame, Samples,
    SensorLayout,
};
use greycard_edit::Edit;
use greycard_engine::stages::{Keep, Purpose, Steps, stages};
use greycard_engine::walk::{
    Hold, Input, Kept, NoHold, NoModels, Place, Stopped, plan, walk, walk_fill_round,
};

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

/// One measurement at a time: the count is the process's.
static ONE: Mutex<()> = Mutex::new(());

/// A Bayer raw `w` by `h`: a ramp whose right quarter clips on green, a
/// disk that clips on green and red, and a fine checker.
fn frame(w: usize, h: usize) -> Arc<RawFrame> {
    let pattern = CfaPattern::rggb();
    let scene = |x: f32, y: f32| {
        let ramp = 200.0 + 2400.0 * x / w as f32;
        let (dx, dy) = (x - w as f32 * 0.6, y - h as f32 * 0.4);
        let r = (h as f32 / 6.0).powi(2);
        let disk = if dx * dx + dy * dy < r { 1200.0 } else { 0.0 };
        let checker = if ((x / 3.0).floor() + (y / 3.0).floor()) as i64 % 2 == 0 {
            150.0
        } else {
            0.0
        };
        ramp + disk + checker
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

fn on() -> Edit {
    let mut edit = Edit {
        highlight_fill: Some("hl-1".into()),
        ..Edit::default()
    };
    edit.sharpen.enabled = false;
    edit
}

/// The window's list for the fill on, and its first round's: the camera
/// picture kept where the fill would run, the fill taken out, each
/// step's place carried over.
fn lists(input: &Input) -> ((Steps, Vec<Place>), (Steps, Vec<Place>)) {
    let full = stages(&on(), &Purpose::Viewport, &input.facts(0), None);
    let places = plan(&full, &Purpose::Viewport, false);
    let at = full.fill_at().expect("the fill is in the list");
    let mut first_places = places.clone();
    first_places.insert(at, Place::Cpu);
    first_places.remove(at + 1);
    let first = full.kept_before_fill().without_fill();
    ((full, places), (first, first_places))
}

/// `f`'s most held at once over what was live before it, and that.
fn peak_of<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let out = f();
    (out, PEAK.load(Ordering::SeqCst) - before, before)
}

/// Gives way inside the fill, at its first ask.
struct Newer;

impl Hold for Newer {
    fn give_way_inside(&self, _: &greycard_engine::stages::Stage) -> bool {
        true
    }
}

/// The figures of one frame: the picture's bytes, a plain walk's peak
/// with the fill, the first round's, and the fill round's over what the
/// frame held before it.
struct Figures {
    picture: usize,
    one_walk: usize,
    first: usize,
    round: usize,
    held: usize,
    camera: u64,
    /// The fill round's peak from the camera picture alone, nothing
    /// else held: what it would be had it let go of nothing.
    alone: usize,
}

fn figures(input: &Input) -> Figures {
    let ((full, places), (first, first_places)) = lists(input);
    let walk_of = |steps: &Steps, places: &[Place], kept: &mut Kept| {
        walk(
            steps,
            places,
            input,
            kept,
            &mut NoModels,
            &mut None,
            &NoHold,
        )
        .expect("walks")
    };
    let (walked, one_walk, _) = peak_of(|| walk_of(&full, &places, &mut Kept::new()));
    let picture = walked
        .taps
        .guide
        .as_ref()
        .map(|_| match &walked.buf {
            greycard_engine::walk::Buf::Working(i) => i.data.len() * 4,
            _ => unreachable!("on the CPU"),
        })
        .unwrap();
    drop(walked);
    let mut kept = Kept::new();
    let (walked, first_peak, _) = peak_of(|| walk_of(&first, &first_places, &mut kept));
    drop(walked);
    assert!(kept.holds(Keep::Camera) && kept.holds(Keep::Base));
    let camera = kept.bytes() - (picture as u64);
    let (walked, round, held) = peak_of(|| {
        walk_fill_round(
            &full,
            &places,
            input,
            &mut kept,
            &mut NoModels,
            &mut None,
            &NoHold,
            (),
        )
        .0
        .expect("the round")
    });
    assert_eq!(walked.from, Some(Keep::Camera), "from the camera picture");
    assert!(!kept.holds(Keep::Camera), "let go once filled");
    drop(walked);
    drop(kept);
    let mut kept = Kept::new();
    walk_of(&first, &first_places, &mut kept);
    for keep in [Keep::Base, Keep::Patched, Keep::PreSharpen, Keep::Developed] {
        kept.take_working(keep);
    }
    let (walked, alone, _) = peak_of(|| {
        walk_fill_round(
            &full,
            &places,
            input,
            &mut kept,
            &mut NoModels,
            &mut None,
            &NoHold,
            (),
        )
        .0
        .expect("the round")
    });
    drop(walked);
    Figures {
        alone,
        picture,
        one_walk,
        first: first_peak,
        round,
        held,
        camera,
    }
}

/// The round from the camera picture holds less than a walk with the
/// fill from the decode: no mosaic and no demosaic. Its peak is in the
/// fill, over what the frame held (the picture without the fill and
/// the camera picture): the camera picture's copy the fill works on and
/// the fill's own analysis, the same as from the camera picture alone.
/// Measured at 1200 by 800: a walk with the fill 9.2 pictures, the
/// round 2.1 over the 2.3 the frame held, 2.1 from the camera picture
/// alone. A given-way round leaves what the frame holds as it was, to
/// the byte.
#[test]
fn the_fill_round_holds_less_than_a_walk_from_the_decode() {
    let _one = ONE.lock().unwrap();
    let input = Input::Raw(frame(1200, 800));
    let f = figures(&input);
    let p = f.picture as f64;
    eprintln!(
        "picture {:.1} MB; a walk with the fill {:.2} pictures; the first round {:.2}; \
         the fill round {:.2} over the {:.2} the frame held, {:.2} alone; \
         the camera picture kept {:.2}",
        p / 1e6,
        f.one_walk as f64 / p,
        f.first as f64 / p,
        f.round as f64 / p,
        f.held as f64 / p,
        f.alone as f64 / p,
        f.camera as f64 / p,
    );
    assert!(f.round < f.one_walk, "{} {}", f.round, f.one_walk);
    assert!((f.round as f64) < 2.5 * p, "{:.2}", f.round as f64 / p);

    // The camera picture: its RGB and its cells, a byte a 2x2 cell.
    assert_eq!(f.camera, (f.picture + 600 * 400) as u64);

    // Given way: what the frame holds, as it was.
    let ((full, places), (first, first_places)) = lists(&input);
    let mut kept = Kept::new();
    walk(
        &first,
        &first_places,
        &input,
        &mut kept,
        &mut NoModels,
        &mut None,
        &NoHold,
    )
    .unwrap();
    let bytes = kept.bytes();
    let base = kept.picture(Keep::Base).cloned().unwrap();
    let gave = walk_fill_round(
        &full,
        &places,
        &input,
        &mut kept,
        &mut NoModels,
        &mut None,
        &Newer,
        (),
    );
    assert!(matches!(gave, (Err(Stopped::GaveWay(_)), Some(()))));
    assert_eq!(kept.bytes(), bytes);
    assert!(Arc::ptr_eq(kept.picture(Keep::Base).unwrap(), &base));
    assert!(kept.holds(Keep::Camera) && kept.fill().is_none());
}

#[test]
#[ignore = "a measurement, printed: run in release"]
fn the_fill_round_at_24_megapixels() {
    let _one = ONE.lock().unwrap();
    let input = Input::Raw(frame(6000, 4000));
    let f = figures(&input);
    let mb = |b: usize| b as f64 / 1_048_576.0;
    eprintln!(
        "24 MP: picture {:.0} MB; a walk with the fill peaks {:.0} MB, the first round {:.0}, \
         the fill round {:.0} over the {:.0} the frame held, {:.0} alone; \
         the camera picture kept {:.0} MB",
        mb(f.picture),
        mb(f.one_walk),
        mb(f.first),
        mb(f.round),
        mb(f.held),
        mb(f.alone),
        mb(f.camera as usize),
    );
}

/// Models that note, as the retouch after the base asks, whether the
/// picture without the fill was let go.
struct Watch {
    unfilled: std::sync::Weak<greycard_core::image::WorkingImage>,
    gone: Option<bool>,
}

impl greycard_engine::walk::Models for Watch {
    fn can_denoise(
        &mut self,
        model: &'static greycard_ai::Model,
    ) -> Result<(), greycard_engine::walk::NoLearned> {
        NoModels.can_denoise(model)
    }

    fn denoise(
        &mut self,
        model: &'static greycard_ai::Model,
        frame: &RawFrame,
        prepared: &greycard_core::develop::Prepared,
    ) -> Result<Vec<f32>, greycard_engine::walk::NoLearned> {
        NoModels.denoise(model, frame, prepared)
    }

    fn fill(
        &mut self,
        _: &greycard_edit::retouch::Patch,
        _: &greycard_core::develop::retouch::Region,
        _: &greycard_core::image::WorkingImage,
    ) -> Option<Vec<f32>> {
        None
    }

    fn could_fill_more(
        &mut self,
        _: &greycard_edit::retouch::Retouch,
        _: &greycard_core::image::WorkingImage,
    ) -> bool {
        false
    }

    fn fills_ahead(
        &mut self,
        _: &greycard_edit::retouch::Retouch,
        _: &greycard_core::image::WorkingImage,
    ) {
        self.gone = Some(self.unfilled.upgrade().is_none());
    }

    fn rasters(
        &mut self,
        _: greycard_engine::stages::Masks,
        _: &Arc<greycard_core::image::WorkingImage>,
        _: Option<greycard_engine::walk::Patched<'_>>,
        _: &greycard_engine::walk::Meta,
    ) -> greycard_engine::walk::Rasters {
        greycard_engine::walk::Rasters::new()
    }
}

/// The picture without the fill is let go once the fill is made, so it
/// and the filled picture are never held at once: gone by the retouch,
/// the first stage after the base.
#[test]
fn the_picture_without_the_fill_goes_before_the_filled_one_is_made() {
    let _one = ONE.lock().unwrap();
    let input = Input::Raw(frame(320, 240));
    let mut edit = on();
    edit.retouch = greycard_edit::retouch::Retouch {
        enabled: true,
        patches: vec![greycard_edit::retouch::Patch {
            id: 1,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.2, 0.2]],
            radius: 0.04,
            ..Default::default()
        }],
    };
    let full = stages(&edit, &Purpose::Viewport, &input.facts(0), None);
    let places = plan(&full, &Purpose::Viewport, false);
    let at = full.fill_at().unwrap();
    let mut first_places = places.clone();
    first_places.insert(at, Place::Cpu);
    first_places.remove(at + 1);
    let first = full.kept_before_fill().without_fill();
    let mut kept = Kept::new();
    walk(
        &first,
        &first_places,
        &input,
        &mut kept,
        &mut NoModels,
        &mut None,
        &NoHold,
    )
    .unwrap();
    let unfilled = Arc::downgrade(kept.picture(Keep::Base).unwrap());
    let mut watch = Watch {
        unfilled,
        gone: None,
    };
    walk_fill_round(
        &full,
        &places,
        &input,
        &mut kept,
        &mut watch,
        &mut None,
        &NoHold,
        (),
    )
    .0
    .unwrap();
    assert_eq!(watch.gone, Some(true));
}
