//! What the camera-space highlight fill holds over its inputs (the
//! camera picture and the codes): counted by the allocator rather than
//! read off the process's resident set. A binary of its own, since the
//! count is the whole process's and one test at a time is the only way
//! to read it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use greycard_core::develop::apply_matrix;
use greycard_core::develop::clipcodes::ClipCodes;
use greycard_core::develop::highlight_fill::net::{FillLook, FillWeights, TENSORS, Tensor};
use greycard_core::develop::highlight_fill::{Camera, FrameCodes, camera_fields, fill_fields};

/// Every byte the process has live, and the most it has had since the
/// mark was last reset.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
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
            let now = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        p
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                let grown = new_size - layout.size();
                let now = LIVE.fetch_add(grown, Ordering::SeqCst) + grown;
                PEAK.fetch_max(now, Ordering::SeqCst);
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

/// The most `f` had live at once over what was live before it.
fn peak_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let out = f();
    (out, PEAK.load(Ordering::SeqCst) - before)
}

/// hl-1's look.
const LOOK: FillLook = FillLook {
    core_whiten: 0.5,
    core_boost: 1.4,
    harmonic_bright: 0.25,
    depth_tie: [12.0, 48.0],
    gain_sigma: 6.0,
    reach: None,
};

/// The oracle fixtures' random-seeded checkpoint: the network's cost
/// does not depend on what it learned.
fn weights() -> FillWeights {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/highlight_fill/weights/"
    );
    let manifest = std::fs::read_to_string(format!("{dir}manifest.txt")).unwrap();
    let tensors = TENSORS
        .iter()
        .map(|(name, _)| {
            let line = manifest
                .lines()
                .find(|l| l.split_whitespace().next() == Some(name))
                .unwrap();
            let shape: Vec<usize> = line
                .split_whitespace()
                .skip(2)
                .map(|d| d.parse().unwrap())
                .collect();
            let bytes = std::fs::read(format!("{dir}{name}.bin")).unwrap();
            let data = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&b| f32::from_le_bytes(b))
                .collect();
            Tensor {
                name: (*name).to_string(),
                shape,
                data,
            }
        })
        .collect();
    FillWeights::from_tensors(tensors).unwrap()
}

/// A warm camera at a ceiling of 2.1 with Canon's headroom.
fn camera() -> Camera {
    let rows = [[1.7f32, -0.5, -0.2], [-0.2, 1.4, -0.2], [0.0, -0.4, 1.4]];
    Camera {
        matrix: std::array::from_fn(|c| std::array::from_fn(|r| rows[r][c])),
        ceiling: 2.1,
        headroom: 1.123,
    }
}

/// A camera picture of `w` × `h` and its Bayer codes: a gradient,
/// lamps scattered over it, and one large clipped disk.
fn frame(w: usize, h: usize) -> (Vec<f32>, ClipCodes) {
    let top = camera().scale();
    let mut disks: Vec<(f32, f32, f32)> = (0..400)
        .map(|k| {
            let k = k as f32;
            let x = (k * 0.618_034).fract() * w as f32;
            let y = (k * 0.414_214).fract() * h as f32;
            (x, y, 3.0 + (k * 0.732_051).fract() * h as f32 / 80.0)
        })
        .collect();
    disks.push((w as f32 / 3.0, h as f32 / 2.0, h as f32 / 4.0));
    let mut rgb = vec![0.0f32; w * h * 3];
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut codes = vec![0u8; cw * ch];
    // Each disk's cells: 3 in its inner part, 2 and then 1 to its rim.
    for (dx, dy, r) in disks {
        let cells = |c: f32, n: usize| {
            ((c - r) / 2.0).max(0.0) as usize..(((c + r) / 2.0).max(0.0) as usize + 1).min(n)
        };
        for cy in cells(dy, ch) {
            for cx in cells(dx, cw) {
                let (x, y) = (2.0 * cx as f32 + 1.0, 2.0 * cy as f32 + 1.0);
                let d = ((x - dx).powi(2) + (y - dy).powi(2)).sqrt() / r;
                let k = match d {
                    d if d < 0.6 => 3,
                    d if d < 0.8 => 2,
                    d if d < 1.0 => 1,
                    _ => 0,
                };
                let at = &mut codes[cy * cw + cx];
                *at = (*at).max(k);
            }
        }
    }
    for y in 0..h {
        for x in 0..w {
            let k = codes[(y / 2) * cw + x / 2];
            let t = (x + y) as f32 / (w + h) as f32;
            let v = if k > 0 { top } else { top * (0.1 + 0.5 * t) };
            rgb[3 * (y * w + x)..3 * (y * w + x) + 3].copy_from_slice(&[
                v * 0.5,
                v * 0.45,
                v * 0.3,
            ]);
        }
    }
    let codes = ClipCodes {
        width: w,
        height: h,
        cell: 2,
        cells_wide: cw,
        cells_high: ch,
        codes,
        clips: [0.987, 0.47, 0.7],
    };
    (rgb, codes)
}

/// The camera-space fill's peak over its inputs, and that of the same
/// fill with its input made whole first, in MB.
fn measure(w: usize, h: usize) -> (f64, f64, usize) {
    let _one = ONE.lock().unwrap();
    let weights = weights();
    let camera = camera();
    let (mut rgb, codes) = frame(w, h);
    let view = FrameCodes::new(&codes, 1).unwrap();
    let (changed, through) = peak_of(|| {
        let fill = camera_fields(
            &rgb,
            w,
            h,
            &camera,
            Some(&view),
            view.clips(),
            &weights,
            &LOOK,
            1.0,
        )
        .expect("a fill");
        let changes = fill.apply(&mut rgb, &view);
        // A bit a pixel, however many changed and wherever.
        assert_eq!(changes.bytes(), h * w.div_ceil(64) * 8);
        changes.count()
    });
    let (rgb, _) = frame(w, h);
    let (_, whole) = peak_of(|| {
        let mut x = rgb.clone();
        apply_matrix(&mut x, camera.matrix);
        let s = camera.scale();
        x.iter_mut().for_each(|v| *v /= s);
        let fields = fill_fields(&x, w, h, &view, view.clips(), &weights, &LOOK, 1.0);
        fields.map(|f| f.bytes())
    });
    let mb = |b: usize| b as f64 / 1_048_576.0;
    (mb(through), mb(whole), changed)
}

/// The fill at 24 MP holds at most this many bytes a pixel over its
/// inputs (12 bytes a pixel of camera RGB, and the codes): the
/// analysis's half-size planes and the fields, measured at 10.9 on this
/// frame. The input made whole first would add its own 12.
const LINE: f64 = 12.5;

#[test]
fn the_camera_fill_holds_no_second_picture() {
    let (w, h) = (6000, 4000);
    let (through, whole, changed) = measure(w, h);
    let per = through * 1_048_576.0 / (w * h) as f64;
    eprintln!(
        "{w}x{h}: {changed} pixels changed, peak +{through:.0} MB ({per:.2} B/px), \
         with the input made whole +{whole:.0} MB"
    );
    assert!(changed > 1_000_000, "{changed}");
    assert!(per < LINE, "{per:.2} bytes a pixel");
    assert!(whole - through > 200.0, "{whole:.0} against {through:.0}");
}

/// 100 MP, reported:
/// `cargo test -p greycard-core --release --test highlight_fill_memory -- --ignored --nocapture`.
#[test]
#[ignore]
fn at_a_hundred_megapixels() {
    let (w, h) = (11648, 8736);
    let (through, whole, changed) = measure(w, h);
    eprintln!(
        "{w}x{h}: {changed} pixels changed, peak +{through:.0} MB ({:.2} B/px), \
         with the input made whole +{whole:.0} MB",
        through * 1_048_576.0 / (w * h) as f64
    );
}
