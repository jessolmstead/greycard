//! What an open frame and a base in the making hold, measured: the
//! check behind the shelf's per-pixel figures ([`crate::memory::Cost`]).
//!
//! Ignored, Linux only (it reads `/proc/self`): on lavapipe the device's
//! memory is the process's, as on unified memory, so the resident set
//! counts both sides and the allocator's report says which is the GPU's.
//!
//!     VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json WGPU_BACKEND=vulkan \
//!     GREYCARD_REQUIRE_GPU=1 cargo test -p greycard-engine --lib \
//!     what_an_open_frame_holds -- --ignored --nocapture

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::prefetch::{Ahead, Fetched, Neighbor, Side, prefetch};
use super::shelf::{Budget, Shelf};
use super::*;

/// A Bayer DNG of our own writing, `width` by `height`.
fn cfa_dng(path: &std::path::Path, width: usize, height: usize) {
    let frame = super::tests::aberrated_frame(width, height);
    let greycard_core::raw::Samples::U16(raw) = &frame.samples else {
        unreachable!("the test frame is 16-bit");
    };
    let samples: Vec<f32> = raw
        .iter()
        .map(|&v| (f32::from(v) - 100.0) / 1000.0)
        .collect();
    let greycard_core::raw::SensorLayout::Cfa(pattern) = &frame.layout else {
        unreachable!("the test frame is Bayer");
    };
    let mosaic = greycard_core::dng::CfaMosaic {
        samples: &samples,
        width,
        height,
        pattern,
    };
    let file = std::fs::File::create(path).unwrap();
    greycard_core::dng::write_cfa_dng(
        std::io::BufWriter::new(file),
        &frame,
        &mosaic,
        [2.0, 1.0, 4.0],
        &Default::default(),
    )
    .unwrap();
}

/// The process's resident bytes now, and its peak since the last
/// [`reset_peak`].
fn resident() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let kb = |key: &str| {
        status
            .lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    (kb("VmRSS:"), kb("VmHWM:"))
}

fn reset_peak() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

/// The device's live allocations, and their bytes by label.
fn on_device(device: &greycard_gpu::wgpu::Device) -> (u64, BTreeMap<String, (usize, u64)>) {
    let Some(report) = device.generate_allocator_report() else {
        return (0, BTreeMap::new());
    };
    let mut by = BTreeMap::new();
    for a in &report.allocations {
        let e = by.entry(a.name.clone()).or_insert((0usize, 0u64));
        e.0 += 1;
        e.1 += a.size;
    }
    (report.total_allocated_bytes, by)
}

/// The device's live allocations by label at their peak while `run`
/// runs, sampled.
type ByLabel = BTreeMap<String, (usize, u64)>;

/// The device's peak allocation while `run` runs, sampled, with its
/// labels then.
fn device_peak<T>(
    device: &greycard_gpu::wgpu::Device,
    run: impl FnOnce() -> T,
) -> (T, u64, ByLabel) {
    let peak = AtomicU64::new(0);
    let at_peak = Mutex::new(ByLabel::new());
    let done = AtomicBool::new(false);
    let out = std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(Ordering::Acquire) {
                let (now, by) = on_device(device);
                if now > peak.fetch_max(now, Ordering::Relaxed) {
                    *at_peak.lock().unwrap() = by;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
        let out = run();
        done.store(true, Ordering::Release);
        out
    });
    (
        out,
        peak.load(Ordering::Relaxed),
        at_peak.into_inner().unwrap(),
    )
}

fn per_px(bytes: u64, pixels: u64) -> f64 {
    bytes as f64 / pixels as f64
}

fn print_device(what: &str, by: &BTreeMap<String, (usize, u64)>, pixels: u64) {
    eprintln!("  {what}, on the device by label:");
    for (name, (n, bytes)) in by {
        if *bytes >= pixels / 4 {
            eprintln!(
                "    {name:<40} x{n:<3} {:>8.1} MB {:>6.2} B/px",
                *bytes as f64 / 1e6,
                per_px(*bytes, pixels)
            );
        }
    }
}

/// The edit measured, from `GREYCARD_MEASURE_EDIT`: the default, or
/// `clarity`, `dehaze`, or `both` (the Detail section's), `mask` (a
/// mask with a Detail of its own), or `range` (a luminance range mask
/// beside Clarity, which reads the picture before the Detail section).
fn measured_edit() -> Edit {
    let mut edit = Edit::default();
    let what = std::env::var("GREYCARD_MEASURE_EDIT").unwrap_or_default();
    match what.as_str() {
        "clarity" => edit.detail.clarity = 0.3,
        "dehaze" => edit.detail.dehaze = 0.3,
        "both" => {
            edit.detail.clarity = 0.3;
            edit.detail.dehaze = 0.3;
        }
        "mask" | "range" => {
            let mut a = greycard_edit::Adjustment {
                id: 1,
                ..Default::default()
            };
            let mut c = greycard_edit::mask::Component::default();
            if what == "range" {
                c.shape = greycard_edit::mask::Shape::LUMINANCE;
                edit.detail.clarity = 0.3;
                a.look.light.exposure = 0.5;
            } else {
                a.look.detail.texture = -0.5;
            }
            a.mask.components.push(c);
            edit.adjustments.push(a);
        }
        _ => {}
    }
    eprintln!(
        "the edit: {}",
        if what.is_empty() { "default" } else { &what }
    );
    edit
}

/// An open frame at rest and a base in the making, measured on a
/// frame of `GREYCARD_MEASURE_SIZE` (`WxH`, 4000x3000 when unset): the
/// open's develop as the window has it (the worker, a device lent,
/// the developed texture held as the viewport holds it), then a
/// prefetch's base of a second frame beside it.
#[test]
#[ignore = "a measurement: run by hand on lavapipe"]
fn what_an_open_frame_holds() {
    use greycard_gpu::wgpu;
    let (width, height) = std::env::var("GREYCARD_MEASURE_SIZE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((4000usize, 3000usize));
    let pixels = (width * height) as u64;
    let dir = crate::testing::scratch_dir("open-memory");
    let (a, b, c) = (
        dir.join("measure-a.dng"),
        dir.join("measure-b.dng"),
        dir.join("measure-c.dng"),
    );
    for path in [&a, &b, &c] {
        cfa_dng(path, width, height);
    }
    let instance = greycard_gpu::instance();
    let Some((device, queue)) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()
            .and_then(|adapter| {
                eprintln!("adapter: {}", adapter.get_info().name);
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("memory measure"),
                    required_limits: adapter.limits(),
                    ..Default::default()
                }))
                .ok()
            })
    else {
        super::tests::skipped("the memory measure");
        return;
    };
    type Shown = (
        Developed,
        Option<Arc<greycard_render::detail::DetailPlanes>>,
    );
    let held: Arc<Mutex<Option<Shown>>> = Arc::new(Mutex::new(None));
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = {
        let held = held.clone();
        let tx = Mutex::new(tx);
        Worker::new(move |o| match o {
            Outcome::Developed {
                generation,
                image,
                planes,
                ..
            } => {
                // As the viewport holds the picture it was given.
                *held.lock().unwrap() = Some((image, planes));
                tx.lock().unwrap().send(generation).unwrap();
            }
            Outcome::Failed { generation, .. } => tx.lock().unwrap().send(generation).unwrap(),
            _ => {}
        })
    };
    worker.shelf_by_test(Budget::Bytes(0));
    // `GREYCARD_MEASURE_CPU`: the develop with no device lent.
    let cpu = std::env::var_os("GREYCARD_MEASURE_CPU").is_some();
    if !cpu {
        worker.set_gpu(&device, &queue);
    }
    let open = |path: &std::path::Path, generation: u64| Job::Open {
        path: path.to_path_buf(),
        version: None,
        edit: measured_edit(),
        generation,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    };
    let wait = std::time::Duration::from_secs(1800);
    let settle = || std::thread::sleep(std::time::Duration::from_millis(500));
    settle();
    let (rss0, _) = resident();
    let (dev0, _) = on_device(&device);
    reset_peak();
    let ((), dev_peak, open_peak) = device_peak(&device, || {
        worker.send(open(&a, 1));
        assert_eq!(rx.recv_timeout(wait).unwrap(), 1);
        // At rest: the settle made, and the sharpen's working set let go
        // where it is kept only through an adjustment.
        let asked = Instant::now();
        while on_device(&device)
            .1
            .keys()
            .any(|k| k.starts_with("sharpen"))
            && asked.elapsed() < std::time::Duration::from_secs(600)
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
    settle();
    print_device("at the open's peak", &open_peak, pixels);
    let (rss1, hwm1) = resident();
    let (dev1, by1) = on_device(&device);
    eprintln!(
        "{width}x{height}, {:.1} MP, {}",
        pixels as f64 / 1e6,
        if held
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|(d, _)| matches!(d, Developed::Texture(_)))
        {
            "the developed picture a texture"
        } else {
            "the developed picture halves"
        }
    );
    let line = |what: &str, all: u64, gpu: u64| {
        eprintln!(
            "  {what:<34} all {:>8.1} MB {:>6.2} B/px; device {:>8.1} MB {:>6.2} B/px; \
             memory {:>8.1} MB {:>6.2} B/px",
            all as f64 / 1e6,
            per_px(all, pixels),
            gpu as f64 / 1e6,
            per_px(gpu, pixels),
            all.saturating_sub(gpu) as f64 / 1e6,
            per_px(all.saturating_sub(gpu), pixels)
        );
    };
    line("the open frame at rest", rss1 - rss0, dev1 - dev0);
    line("the open's peak", hwm1 - rss0, dev_peak - dev0);
    print_device("the open frame at rest", &by1, pixels);

    // A prefetch's base of the second frame beside it, on a context of
    // its own on the same device, the worker idle.
    let ctx = greycard_gpu::Context::from_device(&device, &queue).unwrap();
    let mut gpu = (!cpu).then_some(ctx);
    settle();
    let (rss2, _) = resident();
    let (dev2, _) = on_device(&device);
    let mut shelf = Shelf::new(Budget::Bytes(u64::MAX));
    let n = Neighbor {
        path: b.clone(),
        edit: measured_edit(),
        turn: 0,
        seed_blend: false,
        side: Side::Next,
        steps: 1,
        read_first: None,
    };
    reset_peak();
    let (fetched, dev_peak, at_peak) = device_peak(&device, || {
        prefetch(&n, None, &[], &mut shelf, &mut gpu, None, &|_| {
            Ahead::Nothing
        })
    });
    assert_eq!(fetched, Fetched::Landed);
    let (_, hwm3) = resident();
    settle();
    let (rss3, _) = resident();
    let (dev3, _) = on_device(&device);
    line("a prefetched base on the shelf", rss3 - rss2, dev3 - dev2);
    line(
        "a prefetch's peak",
        hwm3 - rss2,
        dev_peak.saturating_sub(dev2),
    );
    eprintln!(
        "  the shelf counts {:.1} MB, {:.2} B/px",
        shelf.bytes() as f64 / 1e6,
        per_px(shelf.bytes(), pixels)
    );
    print_device("at the prefetch's peak", &at_peak, pixels);
    drop(gpu);
    drop(shelf);

    // Another frame opened from the first, as a step makes it.
    settle();
    let (rss4, _) = resident();
    let (dev4, _) = on_device(&device);
    reset_peak();
    let ((), dev_peak, at_peak) = device_peak(&device, || {
        worker.send(open(&c, 2));
        assert_eq!(rx.recv_timeout(wait).unwrap(), 2);
    });
    let (_, hwm5) = resident();
    settle();
    let (rss5, _) = resident();
    let (dev5, _) = on_device(&device);
    line(
        "a step's open, at rest after",
        rss5.saturating_sub(rss4),
        dev5.saturating_sub(dev4),
    );
    line(
        "a step's open, its peak",
        hwm5.saturating_sub(rss4),
        dev_peak.saturating_sub(dev4),
    );
    print_device("at the step's peak", &at_peak, pixels);
    worker.stop();
    drop(held);
    crate::testing::remove_dir_retry(&dir);
}
