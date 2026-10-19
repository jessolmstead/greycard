use super::scopes::ANALYSIS_WIDTH;
use super::*;
use crate::finish::Baked;
use greycard_core::image::WorkingImage;
use greycard_edit::brush::{RASTER_WIDTH, WIDE_RASTER_WIDTH};
use greycard_edit::mask::{Component, Mode, Shape};
use greycard_edit::{Look, Mask};
use rayon::prelude::*;

/// Both shaders parse and validate. Nothing else in `cargo test`
/// reads the WGSL — the pipelines are built against a real
/// device, which the test machines have no business needing — so
/// without this a typo in `viewport.wgsl` is found by a black
/// viewport rather than by the suite. `Capabilities::all` because
/// this is asking whether the source is a well-formed module,
/// not which device would accept it; the device says that.
#[test]
fn the_shaders_parse_and_validate() {
    for (name, source) in [
        ("viewport.wgsl", include_str!("../viewport.wgsl")),
        ("scope.wgsl", include_str!("../scope.wgsl")),
    ] {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}:\n{}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

/// A device of our own, off any window; none without an adapter,
/// and the test says so rather than passing in silence.
fn device(what: &str) -> Option<(gpu::Device, gpu::Queue)> {
    let instance = greycard_gpu::instance();
    let got = pollster::block_on(instance.request_adapter(&gpu::RequestAdapterOptions {
        power_preference: gpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()
    .and_then(|adapter| {
        pollster::block_on(adapter.request_device(&gpu::DeviceDescriptor {
            label: Some("mask test"),
            ..Default::default()
        }))
        .ok()
    });
    if got.is_none() {
        // Under CI the skip is a failure: the runner installs a
        // software adapter so that these tests run, and a run
        // that finds none has lost it, not earned a pass.
        assert!(
            std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
            "{what} has no GPU to run on and GREYCARD_REQUIRE_GPU is set"
        );
        eprintln!("SKIPPED: {what} has no GPU to run on");
        println!("SKIPPED: {what} has no GPU to run on");
    }
    got
}

/// A local with `shapes` joined in order and nothing to its look.
fn local_of(shapes: Vec<(Shape, Mode)>) -> Local {
    Local {
        baked: Baked::of(&Look::default()),
        mask: Mask {
            components: shapes
                .into_iter()
                .map(|(shape, mode)| Component {
                    shape,
                    mode,
                    ..Default::default()
                })
                .collect(),
            invert: false,
        },
        enabled: true,
        rasters: Vec::new(),
    }
}

/// How far the shader's weights for each of `locals` over `image`
/// at `exposure` stops of global exposure are from the CPU's
/// (`Local::weight_sampled` on `finish::sample`): the largest and
/// the mean difference, in weight, over every pixel and local.
/// The CPU is handed the picture the GPU has, rounded to half
/// floats, so what is measured is the two implementations; the
/// read back is eight bits, so half a level (0.002) is the floor.
/// With `covered`, every mask must take in some of the picture, so
/// that two blank answers cannot agree.
fn gpu_against_cpu(
    device: &gpu::Device,
    queue: &gpu::Queue,
    image: &WorkingImage,
    locals: &[Local],
    exposure: f32,
    covered: bool,
) -> (f32, f32) {
    use crate::finish::{local_ab, sample};
    let (w, h) = (image.width, image.height);
    let halves = crate::worker::Halves::from_image(image, None);
    let seen = WorkingImage {
        width: w,
        height: h,
        data: image
            .data
            .iter()
            .map(|v| half::f16::from_f32(*v).to_f32())
            .collect(),
    };
    let mut renderer = Renderer::new(device, queue);
    renderer.upload(&halves);
    let ab = local_ab(&seen);
    let stops = exposure;
    let mut worst = 0.0f32;
    let mut sum = 0.0f64;
    for (k, local) in locals.iter().enumerate() {
        let mut view = View::blank();
        view.center = (w as f32 / 2.0, h as f32 / 2.0);
        view.plane = (w as f32, h as f32);
        view.frame_size = (w as f32, h as f32);
        view.light.exposure = exposure;
        view.locals = locals.to_vec();
        view.show_mask = Some(k);
        view.mask_alone = true;
        let target = renderer.render(w as u32, h as u32, &view).0;
        let shown = renderer.read_back(&target).expect("read back");
        // The largest difference, their sum, how many pixels are
        // more than a level out, and how many the CPU has over half
        // in and part in, so a check of two blank pictures cannot
        // pass.
        let (local_worst, local_sum, over, full, part) = (0..h)
            .into_par_iter()
            .map(|y| {
                let mut t = (0.0f32, 0.0f64, 0usize, 0usize, 0usize);
                for x in 0..w {
                    let i = y * w + x;
                    let px = seen.pixel(x, y);
                    let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32);
                    let cpu = local.weight_sampled(u, v, Some(sample(px, Some(ab[i]), stops)));
                    let gpu = f32::from(shown.get_pixel(x as u32, y as u32)[0]) / 255.0;
                    let d = (cpu - gpu).abs();
                    if d > t.0 && std::env::var_os("GREYCARD_MASK_WORST").is_some() {
                        let s = sample(px, Some(ab[i]), stops);
                        eprintln!("  worse: {d:.4} at {x},{y} px {px:?} cpu {cpu} gpu {gpu} {s:?}");
                    }
                    t.0 = t.0.max(d);
                    t.1 += f64::from(d);
                    t.2 += usize::from(d > 1.5 / 255.0);
                    t.3 += usize::from(cpu > 0.5);
                    t.4 += usize::from(cpu > 0.0 && cpu < 1.0);
                }
                t
            })
            .reduce(
                || (0.0, 0.0, 0, 0, 0),
                |a, b| (a.0.max(b.0), a.1 + b.1, a.2 + b.2, a.3 + b.3, a.4 + b.4),
            );
        eprintln!(
            "mask {k}: {:.1}% over half, {:.1}% part in, max difference {local_worst:.4}, \
             {over} pixels more than a level out",
            100.0 * full as f32 / (w * h) as f32,
            100.0 * part as f32 / (w * h) as f32
        );
        assert!(
            !covered || (full > w * h / 200 && part > w * h / 200),
            "mask {k} is blank"
        );
        worst = worst.max(local_worst);
        sum += local_sum;
    }
    (worst, (sum / (w * h * locals.len()) as f64) as f32)
}

/// The masks the checks run: a luminance window, a color one at
/// the skin preset and one at a blue, and each kind intersected
/// with a drawn gradient: the skin, which a real frame of browns
/// and reds has plenty of, and the lightness from 30 up.
fn range_locals() -> Vec<Local> {
    vec![
        local_of(vec![(
            Shape::Luminance {
                low: 0.45,
                high: 0.8,
                low_feather: 0.1,
                high_feather: 0.05,
            },
            Mode::Add,
        )]),
        local_of(vec![(Shape::skin(), Mode::Add)]),
        local_of(vec![(Shape::color_at(250.0), Mode::Add)]),
        local_of(vec![
            (
                Shape::Linear {
                    from: [0.0, 0.7],
                    to: [0.0, 0.0],
                },
                Mode::Add,
            ),
            (Shape::skin(), Mode::Intersect),
        ]),
        local_of(vec![
            (
                Shape::Linear {
                    from: [0.0, 0.0],
                    to: [0.0, 0.8],
                },
                Mode::Add,
            ),
            (
                Shape::Luminance {
                    low: 0.3,
                    high: 1.0,
                    low_feather: 0.1,
                    high_feather: 0.0,
                },
                Mode::Intersect,
            ),
        ]),
    ]
}

/// A field that runs every hue across and every lightness down,
/// with a little noise so the local mean has something to do.
fn field() -> WorkingImage {
    let (w, h) = (360usize, 240usize);
    let ok = greycard_core::color::Oklab::for_working_space();
    let from_lms = greycard_core::color::invert3(ok.to_lms).unwrap();
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let l = 0.1 + 0.85 * y as f32 / h as f32;
            let hue = (x as f32).to_radians();
            let chroma = 0.12 * (0.5 + 0.5 * ((x * 7 + y * 3) as f32 * 0.37).sin());
            let lab = [l, chroma * hue.cos(), chroma * hue.sin()];
            let lms = greycard_core::color::apply3(&greycard_core::color::LAB_TO_LMS, lab)
                .map(|v| v * v * v);
            data.extend(greycard_core::color::apply3(&from_lms, lms).map(|v| v.max(0.0)));
        }
    }
    WorkingImage {
        width: w,
        height: h,
        data,
    }
}

/// The scopes' bins as the compute pass makes them, for `scope`
/// weighed by `weigh_by`: `analyze` until its read back lands.
fn gpu_bins(
    renderer: &mut Renderer,
    view: &View,
    scope: Scope,
    weigh_by: Option<usize>,
) -> Vec<u32> {
    for _ in 0..100 {
        let (bins, in_flight) = renderer.analyze(view, scope, weigh_by);
        if !in_flight
            && let Some((taken, bins)) = bins
            && taken == scope
        {
            return bins.to_vec();
        }
        let _ = renderer.device.poll(gpu::PollType::wait_indefinitely());
    }
    panic!("the scope's read back never landed");
}

/// The scopes weighed by a mask, on the GPU, against the CPU's
/// reference (`scope::weighted_bins`) over the same analysis
/// picture and coverage: the picture drawn through the view as
/// `analyze` draws it and read back, its alpha checked against the
/// mask's own weight, then binned both ways. Unweighted, the bins
/// are the unweighted reference's and the picture is the same with
/// the weighing on or off.
#[test]
fn the_weighted_scopes_are_the_cpus() {
    let Some((device, queue)) = device("the weighted scopes' GPU check") else {
        return;
    };
    let image = field();
    let (w, h) = (image.width, image.height);
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    let mut view = View::blank();
    view.center = (w as f32 / 2.0, h as f32 / 2.0);
    view.plane = (w as f32, h as f32);
    view.frame_size = (w as f32, h as f32);
    // A gradient across the top, the skin, and a window above
    // anything the field holds, which takes in nothing.
    let mut locals = vec![
        local_of(vec![(
            Shape::Linear {
                from: [0.2, 0.0],
                to: [0.8, 0.0],
            },
            Mode::Add,
        )]),
        local_of(vec![(Shape::skin(), Mode::Add)]),
        local_of(vec![(
            Shape::Luminance {
                low: 1.5,
                high: 2.0,
                low_feather: 0.0,
                high_feather: 0.0,
            },
            Mode::Add,
        )]),
    ];
    locals[0].baked.light.exposure = 0.5;
    view.locals = locals;
    let _ = renderer.render(w as u32, h as u32, &view).0;
    // The analysis picture as `analyze` draws it.
    let aw = ANALYSIS_WIDTH;
    let ah = (aw * h as u32 / w as u32).max(1);
    let whole = |weigh_by: Option<usize>, show: Option<usize>| View {
        zoom: aw as f32 / w as f32,
        center: (w as f32 / 2.0, h as f32 / 2.0),
        weigh_by,
        show_mask: show,
        mask_alone: show.is_some(),
        ..view.clone()
    };
    let mut shot = |v: &View| {
        let t = renderer.render(aw, ah, v).0;
        renderer.read_back(&t).expect("read back")
    };
    let plain = shot(&whole(None, None));
    let (aw, ah) = (aw as usize, ah as usize);
    let rgb = |img: &image::RgbaImage| -> Vec<[f32; 3]> {
        img.pixels()
            .map(|p| [0, 1, 2].map(|c| f32::from(p[c]) / 255.0))
            .collect()
    };
    assert!(plain.pixels().all(|p| p[3] == 0xff), "alpha is 1 unweighed");
    let pixels = rgb(&plain);
    let mut shots = Vec::new();
    for k in 0..view.locals.len() {
        let weighed = shot(&whole(Some(k), None));
        // The weighing changes the alpha and nothing else.
        assert_eq!(rgb(&weighed), pixels, "mask {k}: the picture moved");
        // And the alpha is the mask's weight, as the mask on show
        // draws it.
        let alone = shot(&whole(None, Some(k)));
        let worst = weighed
            .pixels()
            .zip(alone.pixels())
            .map(|(a, b)| a[3].abs_diff(b[0]))
            .max()
            .unwrap_or(0);
        assert!(worst <= 1, "mask {k}: alpha {worst} levels from the mask");
        let coverage: Vec<f32> = weighed.pixels().map(|p| f32::from(p[3]) / 255.0).collect();
        shots.push(coverage);
    }
    // The first two take in part of the picture and the third none.
    let cover = |c: &[f32]| c.iter().sum::<f32>() / c.len() as f32;
    assert!(
        (0.1..0.9).contains(&cover(&shots[0])),
        "{}",
        cover(&shots[0])
    );
    assert!(
        (0.005..0.9).contains(&cover(&shots[1])),
        "{}",
        cover(&shots[1])
    );
    assert_eq!(cover(&shots[2]), 0.0);
    // The binning is exact arithmetic on the same bytes, and the
    // histogram and waveforms agree to the count; a vectorscope
    // cell edge, where the shader's floating point rounds a last
    // bit apart from the CPU's, moves the odd one, weighed or not
    // (2 counts on Vulkan here, before the weighing too).
    let compare = |what: &str, gpu: &[u32], cpu: &[u32]| {
        let off: u64 = gpu
            .iter()
            .zip(cpu)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        let mass: u64 = cpu.iter().map(|&n| u64::from(n)).sum();
        eprintln!("{what}: {off} of {mass} apart");
        assert!(off <= 8, "{what}: {off} of {mass} apart");
    };
    for scope in Scope::ALL {
        let gpu = gpu_bins(&mut renderer, &view, scope, None);
        let cpu = scope::bins(scope, &pixels, aw, ah);
        compare(&format!("{scope:?} unweighed"), &gpu, &cpu);
        if scope != Scope::Vector {
            assert_eq!(gpu, cpu, "{scope:?} unweighed");
        }
        for (k, coverage) in shots.iter().enumerate() {
            let gpu = gpu_bins(&mut renderer, &view, scope, Some(k));
            let cpu = scope::weighted_bins(scope, &pixels, coverage, aw, ah);
            compare(&format!("{scope:?} weighed by mask {k}"), &gpu, &cpu);
            if scope != Scope::Vector {
                assert_eq!(gpu, cpu, "{scope:?} weighed by mask {k}");
            }
            if k == 2 {
                assert!(gpu.iter().all(|&n| n == 0), "{scope:?}: nothing selected");
            }
        }
    }
}

/// The color the readout under the pointer reads is the analysis
/// picture's at that point of the frame, or over a reach one of the
/// box's own pixels, and it is in the bins the compute pass made of
/// that picture where `scope::marks` puts it: each channel's
/// histogram level, the waveform's column of that pixel, the
/// vectorscope's cell. So no mark is ever drawn off the trace. A new
/// image reads nothing until its own analysis is back.
#[test]
fn the_color_under_the_pointer_is_in_the_scopes_bins() {
    let Some((device, queue)) = device("the readout's GPU check") else {
        return;
    };
    let image = field();
    let (w, h) = (image.width, image.height);
    let mut renderer = Renderer::new(&device, &queue);
    let halves = crate::worker::Halves::from_image(&image, None);
    renderer.upload(&halves);
    assert_eq!(
        renderer.analyzed_at(0.5, 0.5, 0.0),
        None,
        "nothing before an analysis"
    );
    let mut view = View::blank();
    view.center = (w as f32 / 2.0, h as f32 / 2.0);
    view.plane = (w as f32, h as f32);
    view.frame_size = (w as f32, h as f32);
    let _ = renderer.render(w as u32, h as u32, &view).0;
    let aw = ANALYSIS_WIDTH;
    let ah = (aw * h as u32 / w as u32).max(1);
    let whole = View {
        zoom: aw as f32 / w as f32,
        center: (w as f32 / 2.0, h as f32 / 2.0),
        ..view.clone()
    };
    let t = renderer.render(aw, ah, &whole).0;
    let picture = renderer.read_back(&t).expect("read back");
    let color = |x: u32, y: u32| {
        let p = picture.get_pixel(x, y);
        [0, 1, 2].map(|i| f32::from(p[i]) / 255.0)
    };
    let points = [
        (0.0, 0.0),
        (0.13, 0.71),
        (0.5, 0.5),
        (0.77, 0.21),
        (0.999, 0.999),
    ];
    // The bins `marks` puts a reading in all have something in them.
    let on_trace = |bins: &[u32], scope: Scope, r: scope::Reading, what: &str| {
        let level = |ch: usize| scope::level(r.color[ch]);
        for ch in 0..3 {
            assert!(
                bins[ch * scope::LEVELS + level(ch)] > 0,
                "{scope:?} {what} histogram"
            );
        }
        match scope.kind() {
            1 => {
                let column = (r.across * scope::COLUMNS as f32) as usize;
                for ch in 0..3 {
                    let at =
                        scope::HIST + (ch * scope::COLUMNS + column) * scope::LEVELS + level(ch);
                    assert!(bins[at] > 0, "{scope:?} {what} channel {ch}");
                }
            }
            2 => {
                let cell = scope::wheel_cell(r.color).expect("on the wheel");
                assert!(bins[scope::HIST + cell] > 0, "{scope:?} {what}");
            }
            _ => {}
        }
    };
    let reach = 2u32;
    for scope in Scope::ALL {
        let bins = gpu_bins(&mut renderer, &view, scope, None);
        for &(u, v) in &points {
            let r = renderer
                .analyzed_at(u, v, 0.0)
                .expect("an analysis was read");
            assert!(r.selected, "unweighed, every pixel is counted");
            let (x, y) = ((u * aw as f32) as u32, (v * ah as f32) as u32);
            assert_eq!(r.color, color(x, y), "({u}, {v})");
            assert_eq!(r.across, (x as f32 + 0.5) / aw as f32, "({u}, {v})");
            on_trace(&bins, scope, r, &format!("({u}, {v})"));
            // Over a reach, one of the box's own pixels, cut at the
            // picture's edge, and in the bins at its own column.
            let r = renderer
                .analyzed_at(u, v, reach as f32 / aw as f32)
                .unwrap();
            let in_box = (y.saturating_sub(reach)..=(y + reach).min(ah - 1)).any(|by| {
                (x.saturating_sub(reach)..=(x + reach).min(aw - 1))
                    .any(|bx| color(bx, by) == r.color && r.across == (bx as f32 + 0.5) / aw as f32)
            });
            assert!(in_box, "({u}, {v}) over a reach: {r:?}");
            on_trace(&bins, scope, r, &format!("({u}, {v}) over a reach"));
        }
    }
    // Weighed by a gradient across the frame: one end is outside the
    // selection, on no trace, and reads the plain color with that
    // said; the other is in it, and its color is in the weighed bins,
    // over a reach as at a point.
    view.locals = vec![local_of(vec![(
        Shape::Linear {
            from: [0.3, 0.0],
            to: [0.7, 0.0],
        },
        Mode::Add,
    )])];
    let plain = |u: f32, v: f32| color((u * aw as f32) as u32, (v * ah as f32) as u32);
    let bins = gpu_bins(&mut renderer, &view, Scope::Rgb, Some(0));
    let ends = [(0.05, 0.5), (0.95, 0.5)];
    let read = ends.map(|(u, v)| renderer.analyzed_at(u, v, 0.0).unwrap());
    assert!(
        read[0].selected != read[1].selected,
        "one end in the selection and one out: {read:?}"
    );
    for (&(u, v), r) in ends.iter().zip(read) {
        // The weighing leaves the picture's colors alone.
        assert_eq!(r.color, plain(u, v), "({u}, {v})");
        if r.selected {
            on_trace(&bins, Scope::Rgb, r, &format!("weighed ({u}, {v})"));
            let r = renderer
                .analyzed_at(u, v, reach as f32 / aw as f32)
                .unwrap();
            assert!(r.selected);
            on_trace(
                &bins,
                Scope::Rgb,
                r,
                &format!("weighed ({u}, {v}) over a reach"),
            );
        }
    }
    // A new image: its own analysis, not the last one's pixels at its
    // shape, is what a reading waits for, and a picture taken before
    // it came in is not kept when it lands.
    view.locals.clear();
    let (_, in_flight) = renderer.analyze(&view, Scope::Rgb, None);
    assert!(in_flight, "an analysis under way");
    renderer.upload(&halves);
    assert_eq!(renderer.analyzed_at(0.5, 0.5, 0.0), None, "after an upload");
    let _ = renderer.device.poll(gpu::PollType::wait_indefinitely());
    let _ = renderer.analyze(&view, Scope::Rgb, None);
    assert_eq!(
        renderer.analyzed_at(0.5, 0.5, 0.0),
        None,
        "the picture taken before the upload"
    );
    let mut read = None;
    for _ in 0..100 {
        let _ = renderer.analyze(&view, Scope::Rgb, None);
        read = renderer.analyzed_at(0.5, 0.5, 0.0);
        if read.is_some() {
            break;
        }
        let _ = renderer.device.poll(gpu::PollType::wait_indefinitely());
    }
    assert!(read.is_some(), "the new image's analysis is read");
}

/// A read back whose map callback wgpu dropped without calling (a
/// surface configure that raced the worker drops what it collected)
/// is let go: the scopes stop waiting on it, unmap the buffer wgpu
/// mapped, and analyze again, and that analysis lands.
#[test]
fn a_lost_scopes_read_back_is_made_again() {
    let Some((device, queue)) = device("the lost read back's check") else {
        return;
    };
    let image = field();
    let (w, h) = (image.width, image.height);
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    let mut view = View::blank();
    view.center = (w as f32 / 2.0, h as f32 / 2.0);
    view.plane = (w as f32, h as f32);
    view.frame_size = (w as f32, h as f32);
    let first = gpu_bins(&mut renderer, &view, Scope::Rgb, None);
    // The next analysis, for an edit of its own, goes out ...
    view.light.exposure = 0.5;
    let (_, in_flight) = renderer.analyze(&view, Scope::Rgb, None);
    assert!(in_flight, "an analysis under way");
    // ... its maps are done, as wgpu-core's poll inside the configure
    // leaves them, and its callbacks are dropped with their senders.
    let _ = renderer.device.poll(gpu::PollType::wait_indefinitely());
    let lost = || {
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx);
        Some(rx)
    };
    renderer.scopes.in_flight = lost();
    renderer.scopes.picture_in_flight = lost();
    // The scopes let it go and send the analysis again at once, so the
    // window's frames do not wait on it for the rest of the session;
    // mapping the staging buffers again would fail had they not been
    // unmapped, costing the scopes one more cycle.
    let (bins, in_flight) = renderer.analyze(&view, Scope::Rgb, None);
    assert!(bins.is_none(), "nothing read from the lost one");
    assert!(in_flight, "the analysis made again");
    assert!(
        renderer.scopes.in_flight.as_ref().unwrap().try_recv()
            != Err(std::sync::mpsc::TryRecvError::Disconnected),
        "a live read back in place of the lost one"
    );
    let again = gpu_bins(&mut renderer, &view, Scope::Rgb, None);
    assert_ne!(again, first, "the bins of the brighter edit");
    assert!(renderer.take_navigator().is_some(), "the navigator's too");
    // And idle once it has landed.
    let (_, in_flight) = renderer.analyze(&view, Scope::Rgb, None);
    assert!(!in_flight);
}

/// The shader's range masks are the CPU's, on the field.
#[test]
fn the_shaders_range_masks_are_the_cpus() {
    let Some((device, queue)) = device("the range masks' GPU check") else {
        return;
    };
    let image = field();
    let (worst, mean) = gpu_against_cpu(&device, &queue, &image, &range_locals(), -0.3, true);
    eprintln!("range masks, GPU against CPU: max {worst:.4}, mean {mean:.5}");
    assert!(worst <= 3.0 / 255.0, "max {worst}");
    assert!(mean <= 0.5 / 255.0, "mean {mean}");
}

/// Two Skies' wide rasters beside a brush's and a learned mask's
/// narrow ones: the narrow ones in their array at their own size, the
/// wide ones in an array of their own, each read inside its own size
/// as `Raster::at` reads it on the CPU. The wide ones' patterns are
/// finer than a narrow texel and differ from each other, so reading a
/// layer in the wrong array, the wrong layer, or at the wrong size is
/// far off; and each local is checked alone.
#[test]
fn wide_rasters_beside_narrow_ones_are_the_cpus() {
    use greycard_edit::brush::Raster;
    let image = field();
    let aspect = image.height as f32 / image.width as f32;
    let pattern = |w: usize, period: f32| {
        let h = greycard_edit::brush::raster_height(w, aspect);
        let data = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let (u, v) = (x as f32 / w as f32, y as f32 / w as f32);
                let ring = 0.5 + 0.5 * ((u * 7.0 + v * 5.0) * period).sin();
                let block = if (u - 0.3).abs() < 0.2 && v < 0.4 {
                    1.0
                } else {
                    0.0
                };
                ((ring * 0.6 + block * 0.4) * 255.0).round() as u8
            })
            .collect();
        RasterRef(Arc::new(Raster::from_data(aspect, w, data)))
    };
    let sky = pattern(WIDE_RASTER_WIDTH, 37.0);
    let other_sky = pattern(WIDE_RASTER_WIDTH, 23.0);
    assert_eq!((sky.0.width, sky.0.height), (4096, 2731));
    let subject = pattern(RASTER_WIDTH, 11.0);
    let mut stroke =
        greycard_edit::brush::Stroke::new(greycard_edit::brush::Op::Add, 0.12, 0.6, 1.0);
    stroke.points = vec![[0.2, 0.2], [0.5, 0.4], [0.8, 0.3]];
    let strokes = vec![stroke];
    let brush = RasterRef(Arc::new(Raster::of(&strokes, aspect)));
    let with = |shape: Shape, raster: &RasterRef| {
        let mut l = local_of(vec![(shape, Mode::Add)]);
        l.rasters = vec![Some(raster.clone())];
        l
    };
    // Narrow, wide, narrow, wide: each array's layers in their order.
    let locals = vec![
        with(Shape::Brush { strokes }, &brush),
        with(Shape::Sky { picks: Vec::new() }, &sky),
        with(Shape::Subject {}, &subject),
        with(Shape::Sky { picks: Vec::new() }, &other_sky),
    ];
    let gpu_locals = super::locals::locals_gpu(&locals);
    assert_eq!(gpu_locals.rasters.len(), 2);
    assert_eq!(gpu_locals.wide.len(), 2);
    assert_eq!(
        super::locals::Brushes::size(&gpu_locals.rasters),
        (2048, 1365),
        "the narrow array keeps the narrow size"
    );
    assert_eq!(super::locals::Brushes::size(&gpu_locals.wide), (4096, 2731));
    let Some((device, queue)) = device("the wide rasters' GPU check") else {
        return;
    };
    let (worst, mean) = gpu_against_cpu(&device, &queue, &image, &locals, 0.0, true);
    eprintln!("narrow and wide rasters, GPU against CPU: max {worst:.4}, mean {mean:.5}");
    assert!(worst <= 3.0 / 255.0, "max {worst}");
    assert!(mean <= 0.5 / 255.0, "mean {mean}");
}

/// A switched-off adjustment's mask can still be shown, and its
/// range shapes read the picture for it even when no switched-on
/// one does: not an empty sample, which painted a window from 0
/// over the whole frame and a color window over none of it.
#[test]
fn a_switched_off_range_mask_shows_what_it_would_take() {
    let off = |shape: Shape| {
        let mut l = local_of(vec![(shape, Mode::Add)]);
        l.enabled = false;
        l
    };
    let dark = off(Shape::Luminance {
        low: 0.0,
        high: 0.3,
        low_feather: 0.0,
        high_feather: 0.05,
    });
    let color = off(Shape::color_at(250.0));
    assert_eq!(range_reads(std::slice::from_ref(&dark)), 1.0);
    assert_eq!(range_reads(&[dark.clone(), color.clone()]), 2.0);
    assert_eq!(range_reads(&[local_of(vec![])]), 0.0);
    let Some((device, queue)) = device("the switched-off range mask's check") else {
        return;
    };
    let image = field();
    for local in [dark, color] {
        let (worst, _) = gpu_against_cpu(
            &device,
            &queue,
            &image,
            std::slice::from_ref(&local),
            0.0,
            true,
        );
        assert!(worst <= 3.0 / 255.0, "max {worst}");
    }
}

/// The mixer reads the mean the color windows made, brought to its
/// own exposure, rather than making it again: a picture with the
/// mixer on and a color window whose look does nothing is the
/// picture without the window, to the byte.
#[test]
fn the_mixer_reads_the_color_windows_mean() {
    let Some((device, queue)) = device("the shared mean's check") else {
        return;
    };
    let image = field();
    let (w, h) = (image.width, image.height);
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    let mut view = View::blank();
    view.center = (w as f32 / 2.0, h as f32 / 2.0);
    view.plane = (w as f32, h as f32);
    view.frame_size = (w as f32, h as f32);
    view.light.exposure = 0.7;
    view.mixer.enabled = true;
    view.mixer.hue[1] = 20.0;
    view.mixer.saturation[5] = 0.6;
    let mut shot = |view: &View| {
        let target = renderer.render(w as u32, h as u32, view).0;
        renderer.read_back(&target).expect("read back")
    };
    let without = shot(&view);
    view.locals = vec![local_of(vec![(Shape::color_at(250.0), Mode::Add)])];
    let with = shot(&view);
    // Within a level: the mean is one computation shared by the
    // window and the mixer, but the mixer re-exposes it, and a
    // backend that contracts multiplies (Metal on the Mac runner)
    // rounds that a last bit apart from Vulkan's.
    let worst = with
        .as_raw()
        .iter()
        .zip(without.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 1,
        "the window changed the mixer's picture by {worst} levels"
    );
    // And the mixer does act, so the check is of something.
    view.mixer.enabled = false;
    assert!(shot(&view) != without);
}

/// A picture read through a turn it has not caught up with draws
/// what the picture developed at that turn draws: the texture
/// turned on the CPU (`orient`, as the develop turns it) and read
/// straight, against the unturned texture read through
/// `source_turn`, for each quarter. The mixer's local mean and
/// the cubic reader both read about a pixel, and a mask sits off
/// center in the turned picture's own positions, so a reader or a
/// mask left in the texture's positions shows.
#[test]
fn a_texture_read_through_a_turn_is_the_turned_texture() {
    use greycard_core::develop::orient;
    use greycard_core::raw::Orientation;
    let Some((device, queue)) = device("the lagging turn's check") else {
        return;
    };
    let image = field();
    // A guide plane whose texels divide the picture, turned with
    // it by the same map, so the texture's guide read through the
    // turn can be told from the turned one's.
    let scale = 8;
    let guide_of = |w: usize, h: usize, at: &dyn Fn(usize, usize) -> f32| {
        let data: Vec<f32> = (0..h / scale)
            .flat_map(|y| (0..w / scale).map(move |x| (x, y)))
            .map(|(x, y)| at(x, y))
            .collect();
        crate::finish::Guide {
            width: w / scale,
            height: h / scale,
            scale,
            range: crate::finish::plane_range(&data),
            data,
        }
    };
    let plain = |x: usize, y: usize| ((x * 7 + y * 3) % 11) as f32 * 0.3 - 1.5;
    let (iw, ih) = (image.width, image.height);
    let guide = guide_of(iw, ih, &plain);
    let mut renderer = Renderer::new(&device, &queue);
    for ((quarters, orientation), cubic) in [
        (1u8, Orientation::Rotate90),
        (2, Orientation::Rotate180),
        (3, Orientation::Rotate270),
    ]
    .into_iter()
    .flat_map(|q| [(q, true), (q, false)])
    {
        let turned = orient(image.clone(), orientation);
        let (w, h) = (turned.width, turned.height);
        let (gw, gh) = (iw / scale, ih / scale);
        let turned_guide = guide_of(w, h, &|x, y| {
            let (tx, ty) = crate::placeholder::texel_of(
                (x as i64, y as i64),
                (w as u32 / scale as u32, h as u32 / scale as u32),
                quarters,
            );
            assert!((tx as usize) < gw && (ty as usize) < gh);
            plain(tx as usize, ty as usize)
        });
        let mut view = View::blank();
        view.light.tone.shadows = 0.6;
        view.light.tone.highlights = -0.5;
        view.center = (w as f32 / 2.0, h as f32 / 2.0);
        view.plane = (w as f32, h as f32);
        view.frame_size = (w as f32, h as f32);
        view.cubic = cubic;
        view.mixer.enabled = true;
        view.mixer.hue[1] = 20.0;
        view.mixer.saturation[5] = 0.6;
        let mut local = local_of(vec![(
            Shape::Radial {
                center: [0.3, 0.2],
                radius: [0.15, 0.1],
                angle: 0.0,
                feather: 0.3,
            },
            Mode::Add,
        )]);
        local.baked.light.exposure = 1.0;
        view.locals = vec![local];
        let mut shot = |source: &WorkingImage, guide: &crate::finish::Guide, lag: u8| {
            renderer.upload(&crate::worker::Halves::from_image(source, None));
            renderer.set_guide(guide);
            let v = View {
                source_turn: lag,
                ..view.clone()
            };
            let target = renderer.render(w as u32, h as u32, &v).0;
            renderer.read_back(&target).expect("read back")
        };
        let developed = shot(&turned, &turned_guide, 0);
        let lagging = shot(&image, &guide, quarters);
        let worst = developed
            .as_raw()
            .iter()
            .zip(lagging.as_raw())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        eprintln!("{quarters} quarters, cubic {cubic}: at most {worst} levels apart");
        assert!(
            worst <= 1,
            "{quarters} quarters, cubic {cubic}: the lagging texture is {worst} levels off"
        );
        // And reading it straight is another picture, so the
        // check is of the turn.
        if quarters % 2 == 0 {
            assert!(shot(&image, &guide, 0) != developed);
            // And the guide is read through the turn too: the
            // texture's own guide read straight is another picture.
            assert!(shot(&image, &turned_guide, quarters) != developed);
        }
    }
}

/// A dropper's read during a turn's lag is the read of the develop
/// at the new turn: the unturned texture sampled at the texel
/// `texel_of` names is the turned texture sampled at the pixel.
#[test]
fn a_pick_through_a_turn_reads_the_pixel_the_turned_develop_has() {
    use greycard_core::develop::orient;
    use greycard_core::raw::Orientation;
    let Some((device, queue)) = device("the lagging pick's check") else {
        return;
    };
    let image = field();
    let mut renderer = Renderer::new(&device, &queue);
    for (lag, orientation) in [
        (1u8, Orientation::Rotate90),
        (2, Orientation::Rotate180),
        (3, Orientation::Rotate270),
    ] {
        let turned = orient(image.clone(), orientation);
        let size = (turned.width as u32, turned.height as u32);
        let points = [
            (0i64, 0i64),
            (7, 3),
            (40, 150),
            (size.0 as i64 - 1, size.1 as i64 - 2),
        ];
        renderer.upload(&crate::worker::Halves::from_image(&turned, None));
        let developed: Vec<_> = points
            .iter()
            .map(|&(x, y)| renderer.sample(x, y, 2).expect("sampled"))
            .collect();
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        for (k, &(x, y)) in points.iter().enumerate() {
            let (tx, ty) = crate::placeholder::texel_of((x, y), size, lag);
            let lagging = renderer.sample(tx, ty, 2).expect("sampled");
            assert!(
                lagging.is_some(),
                "{lag} quarters at {x},{y}: off the picture"
            );
            assert_eq!(lagging, developed[k], "{lag} quarters at {x},{y}");
        }
    }
}

/// The Light section switched off keeps the base curve on the GPU
/// as `finish_pixel` does (issue #9, where off took the curve away
/// and the picture got darker). The check is against the CPU: the
/// view for the section off with sliders set, through
/// `effective()`, draws what `finish_pixel` makes of the default
/// edit, curve and all. The same sliders on are drawn first, so a
/// view that ignored them could not pass for one that undid them.
#[test]
fn the_light_switch_off_keeps_the_base_curve_on_the_gpu() {
    let Some((device, queue)) = device("the light switch's check") else {
        return;
    };
    let image = field();
    let (w, h) = (image.width, image.height);
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    let mut view = View::blank();
    view.center = (w as f32 / 2.0, h as f32 / 2.0);
    view.plane = (w as f32, h as f32);
    view.frame_size = (w as f32, h as f32);
    let mut shot = |view: &View| {
        let target = renderer.render(w as u32, h as u32, view).0;
        renderer.read_back(&target).expect("read back")
    };
    let mut set = Light {
        exposure: 1.2,
        ..Light::default()
    };
    set.tone.contrast = 1.4;
    set.tone.whites = 0.5;
    set.tone.blacks = -0.1;
    view.light = set;
    let lit = shot(&view);
    set.enabled = false;
    view.light = set.effective();
    let off = shot(&view);
    assert!(lit != off, "the sliders act, so the check is of something");
    // And that picture is the CPU's, to a level or so: sRGB, the
    // renderer's output until told otherwise, and the display
    // table the identity.
    let seen: Vec<f32> = image
        .data
        .iter()
        .map(|v| half::f16::from_f32(*v).to_f32())
        .collect();
    let global = Baked::global(&greycard_edit::Edit::default(), Source::SCENE);
    let to_out = crate::export::Space::Srgb.matrix();
    let mut worst = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 3;
            let px = [seen[i], seen[i + 1], seen[i + 2]];
            let cpu = crate::finish::finish_pixel(px, &global, &[], &to_out);
            let gpu = off.get_pixel(x as u32, y as u32);
            for k in 0..3 {
                worst = worst.max((cpu[k] - f32::from(gpu[k]) / 255.0).abs());
            }
        }
    }
    eprintln!("light off, GPU against CPU: max difference {worst:.4}");
    assert!(worst < 2.5 / 255.0, "{worst}");
}

/// AgX on the GPU is `agx::tone_agx` on the CPU, over the parity
/// frame's primaries, near blacks and highlights past white, at
/// exposures from three stops down to four up: the switch reaches
/// the shader through `with_look`, and both sides read the same
/// field of the edit.
#[test]
fn the_agx_curve_on_the_gpu_is_the_cpus() {
    a_display_curve_on_the_gpu_is_the_cpus(greycard_edit::DisplayCurve::Agx, "the AgX curve");
}

/// The gate between a fitted look table and a picture's display
/// curve, on both sides: a table fitted under AgX applies to a
/// picture on AgX and is no look at all on one on per channel, the
/// GPU's picture then being, level for level, its picture with no
/// look; a general table applies under both; and wherever a table
/// applies the GPU is the CPU's finish with it. The gate is one
/// decision (`look::gate`) made before either side is handed a
/// table, which is what the agreement rests on.
#[test]
fn a_fitted_table_is_off_under_another_curve_on_both_sides() {
    use greycard_edit::DisplayCurve;
    let Some((device, queue)) = device("the fitted table's gate") else {
        return;
    };
    let image = parity_frame();
    let (w, h) = (image.width, image.height);
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    let seen = WorkingImage {
        width: w,
        height: h,
        data: image
            .data
            .iter()
            .map(|v| half::f16::from_f32(*v).to_f32())
            .collect(),
    };
    let to_out = crate::export::Space::Srgb.matrix();
    let tagged = |comments: &[&str]| {
        let mut t = (*look_tables()[0]).clone();
        t.comments = comments.iter().map(|c| c.to_string()).collect();
        Arc::new(t)
    };
    let fitted = tagged(&[greycard_edit::look::FITTED, "display_curve: agx"]);
    let general = tagged(&["made by a grading application"]);
    let mut render = |edit: &greycard_edit::Edit, look: Option<&lut::Look>| {
        renderer.set_look(look);
        renderer.set_output(to_out);
        let view = View {
            center: (w as f32 / 2.0, h as f32 / 2.0),
            plane: (w as f32, h as f32),
            frame_size: (w as f32, h as f32),
            ..View::with_look(edit)
        };
        let target = renderer.render(w as u32, h as u32, &view).0;
        let shown = renderer.read_back(&target).expect("read back");
        let gpu: Vec<u8> = shown.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let cpu = crate::finish::finish_with(
            &seen,
            None,
            &Baked::global(edit, Source::SCENE),
            &[],
            |x, y| ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32),
            |_, _| (0.0, None),
            None,
            None,
            look.filter(|l| !l.is_off()),
            &to_out,
            |v: f32| v,
        );
        (cpu, gpu)
    };
    let worst = |cpu: &[f32], gpu: &[u8]| {
        cpu.iter()
            .zip(gpu)
            .map(|(c, g)| (c - f32::from(*g) / 255.0).abs())
            .fold(0.0f32, f32::max)
    };
    let mean_levels = |a: &[u8], b: &[u8]| {
        a.iter()
            .zip(b)
            .map(|(x, y)| (f64::from(*x) - f64::from(*y)).abs())
            .sum::<f64>()
            / a.len() as f64
    };
    for curve in DisplayCurve::ALL {
        let edit = greycard_edit::Edit {
            display_curve: curve,
            ..Default::default()
        };
        let (bare_cpu, bare_gpu) = render(&edit, None);
        for (table, what) in [(&fitted, "fitted under AgX"), (&general, "general")] {
            let resolved = lut::Look::new(table.clone(), 1.0).unwrap();
            let look = greycard_edit::look::gate(Some(resolved), curve);
            let applies = what == "general" || curve == DisplayCurve::Agx;
            assert_eq!(look.is_some(), applies, "{what} under {curve:?}");
            let (cpu, gpu) = render(&edit, look.as_ref());
            let d = worst(&cpu, &gpu);
            let moved = mean_levels(&gpu, &bare_gpu);
            eprintln!(
                "{what} under {curve:?}: applied {applies}, GPU against CPU {:.2} levels, \
                 {moved:.2} levels from no look on average",
                d * 255.0
            );
            assert!(d < 2.5 / 255.0, "{what} under {curve:?}: {d}");
            if applies {
                assert!(moved > 1.0, "{what} under {curve:?} changed nothing");
            } else {
                assert_eq!(gpu, bare_gpu, "{what} under {curve:?} on the GPU");
                assert_eq!(cpu, bare_cpu, "{what} under {curve:?} on the CPU");
            }
        }
    }
}

fn a_display_curve_on_the_gpu_is_the_cpus(curve: greycard_edit::DisplayCurve, what: &str) {
    let Some((device, queue)) = device(&format!("{what}'s check")) else {
        return;
    };
    let image = parity_frame();
    let (w, h) = (image.width, image.height);
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    let seen: Vec<f32> = image
        .data
        .iter()
        .map(|v| half::f16::from_f32(*v).to_f32())
        .collect();
    let to_out = crate::export::Space::Srgb.matrix();
    let mut edit = greycard_edit::Edit {
        display_curve: curve,
        ..Default::default()
    };
    // At the default baseline and at a body's own, whose display white
    // and AgX's fit reach the shader through its uniforms.
    let (make, model) = crate::testing::GFX;
    for source in [Source::SCENE, Source::raw(make, model)] {
        let mut worst = (0.0f32, [0.0f32; 3], [0.0f32; 3], [0.0f32; 3], 0.0f32);
        // The signed difference's mean and its root mean square over
        // the channels not at either end, in levels: the GPU is read
        // back in 8 bits, so the square root of a twelfth (0.29) is
        // the quantization's own, and a mean well under a hundredth of
        // a level says the curve on the GPU has no bias against the
        // CPU's that the quantization could hide.
        let (mut sum, mut sq, mut n) = (0.0f64, 0.0f64, 0usize);
        for stops in [-3.0f32, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0] {
            edit.light.exposure = stops;
            let view = View {
                center: (w as f32 / 2.0, h as f32 / 2.0),
                plane: (w as f32, h as f32),
                frame_size: (w as f32, h as f32),
                source,
                ..View::with_look(&edit)
            };
            let target = renderer.render(w as u32, h as u32, &view).0;
            let shown = renderer.read_back(&target).expect("read back");
            let global = Baked::global(&edit, source);
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) * 3;
                    let px = [seen[i], seen[i + 1], seen[i + 2]];
                    let cpu = crate::finish::finish_pixel(px, &global, &[], &to_out);
                    let gpu = shown.get_pixel(x as u32, y as u32);
                    let gpu = [0, 1, 2].map(|k| f32::from(gpu[k]) / 255.0);
                    for k in 0..3 {
                        let d = (cpu[k] - gpu[k]).abs();
                        if d > worst.0 {
                            worst = (d, px, cpu, gpu, stops);
                        }
                        if cpu[k] > 0.02 && cpu[k] < 0.98 {
                            let signed = f64::from(gpu[k] - cpu[k]) * 255.0;
                            sum += signed;
                            sq += signed * signed;
                            n += 1;
                        }
                    }
                }
            }
        }
        eprintln!(
            "{what} from {source:?}, GPU against CPU: {worst:?}; over {n} channels mean {:+.4} levels, rms {:.3}",
            sum / n as f64,
            (sq / n as f64).sqrt()
        );
        assert!(worst.0 < 1.5 / 255.0, "{source:?}: {worst:?}");
    }
}

/// How far each display curve's value on the GPU is from the CPU's
/// before the point curves, which an 8-bit read back cannot show on
/// its own: a master point curve rising from 0 to 1 across a
/// fiftieth of the encoded range magnifies every pixel that lands
/// in it by fifty to seventy-five, and that curve inverted on the
/// GPU's level gives the pixel's encoded value to a few
/// hundred-thousandths. The output space is Rec.2020, the working
/// space, so the level is the curve's own value. Over the parity
/// frame at three exposures and five windows, for each curve: how
/// many channels landed in a window, the root mean square and the
/// largest gap, in levels of the encoded value. The random parity
/// test finds such a gap only when a drawn curve is this steep
/// where a pixel lands, and then reads it as whole levels; this
/// says how big the gap is for every pixel, whichever curve.
#[test]
fn a_display_curves_gap_before_the_point_curves() {
    use greycard_edit::curve::lookup;
    let Some((device, queue)) = device("the display curves' gap") else {
        return;
    };
    let image = parity_frame();
    let (w, h) = (image.width, image.height);
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    renderer.set_output(crate::export::Space::Rec2020.matrix());
    let seen: Vec<f32> = image
        .data
        .iter()
        .map(|v| half::f16::from_f32(*v).to_f32())
        .collect();
    let mut report = Vec::new();
    for curve in greycard_edit::DisplayCurve::ALL {
        let (mut sq, mut sum, mut n, mut worst) = (0.0f64, 0.0f64, 0usize, 0.0f32);
        for window in [0.1f32, 0.3, 0.5, 0.7, 0.9] {
            for stops in [-2.0f32, 0.0, 2.0] {
                let mut edit = greycard_edit::Edit {
                    display_curve: curve,
                    ..Default::default()
                };
                edit.light.exposure = stops;
                edit.curves.rgb = vec![
                    [0.0, 0.0],
                    [window - 0.01, 0.0],
                    [window + 0.01, 1.0],
                    [1.0, 1.0],
                ];
                let view = View {
                    center: (w as f32 / 2.0, h as f32 / 2.0),
                    plane: (w as f32, h as f32),
                    frame_size: (w as f32, h as f32),
                    ..View::with_look(&edit)
                };
                let target = renderer.render(w as u32, h as u32, &view).0;
                let shown = renderer.read_back(&target).expect("read back");
                let global = Baked::global(&edit, Source::SCENE);
                let look = edit.look();
                for y in 0..h {
                    for x in 0..w {
                        let i = (y * w + x) * 3;
                        let px = [seen[i], seen[i + 1], seen[i + 2]];
                        let before = crate::finish::pick(
                            px,
                            &look.light,
                            &look.mixer,
                            &look.color,
                            &edit.bw,
                            &look.tint,
                            &global.curves,
                            Source::SCENE,
                            curve,
                        )
                        .encoded;
                        let gpu = shown.get_pixel(x as u32, y as u32);
                        for k in 0..3 {
                            let e = before[k];
                            let level = f32::from(gpu[k]) / 255.0;
                            if (e - window).abs() > 0.008 || !(0.02..0.98).contains(&level) {
                                continue;
                            }
                            // The curve inverted on the GPU's level, by
                            // bisection over the window, where it rises.
                            let (mut lo, mut hi) = (window - 0.011, window + 0.011);
                            for _ in 0..40 {
                                let mid = 0.5 * (lo + hi);
                                if lookup(&global.curves, k, mid) < level {
                                    lo = mid;
                                } else {
                                    hi = mid;
                                }
                            }
                            let gap = (0.5 * (lo + hi) - e) * 255.0;
                            sq += f64::from(gap * gap);
                            sum += f64::from(gap);
                            n += 1;
                            worst = worst.max(gap.abs());
                        }
                    }
                }
            }
        }
        report.push((
            curve,
            n,
            (sq / n.max(1) as f64).sqrt(),
            sum / n.max(1) as f64,
            worst,
        ));
    }
    eprintln!("| curve | channels | rms gap | mean gap | largest gap |");
    for (curve, n, rms, mean, worst) in &report {
        eprintln!("| {curve:?} | {n} | {rms:.4} | {mean:+.4} | {worst:.4} |");
    }
    // NVIDIA and lavapipe both: an rms of 0.006 levels and a largest
    // of 0.02 for each of the three curves, which is the chain's
    // float rounding; a tenth of a level is five times that.
    for (curve, n, _, _, worst) in &report {
        assert!(*n > 100, "{curve:?}: only {n} channels landed in a window");
        assert!(
            *worst < 0.1,
            "{curve:?}: {worst} levels before the point curves"
        );
    }
}

/// Milliseconds a 4K viewport frame takes on this device with each
/// display curve, the default edit otherwise; printed, for the
/// notes. The frame is the parity frame drawn at 3840 by 2160, so
/// the cost is the shader's per pixel and not the texture's.
#[test]
#[ignore = "a timing, run by hand in release"]
fn the_cost_of_a_4k_frame() {
    let Some((device, queue)) = device("the 4K frame's timing") else {
        return;
    };
    let image = parity_frame();
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    let (w, h) = (3840u32, 2160u32);
    for curve in greycard_edit::DisplayCurve::ALL {
        let edit = greycard_edit::Edit {
            display_curve: curve,
            ..Default::default()
        };
        // The renderer draws again only for another view, so each
        // frame moves the exposure by a thousandth of a stop.
        let mut time = |frames: u32| {
            let start = std::time::Instant::now();
            for i in 0..frames {
                let mut edit = edit.clone();
                edit.light.exposure = i as f32 * 1e-3;
                let view = View {
                    center: (image.width as f32 / 2.0, image.height as f32 / 2.0),
                    plane: (image.width as f32, image.height as f32),
                    frame_size: (w as f32, h as f32),
                    ..View::with_look(&edit)
                };
                let (target, _) = renderer.render(w, h, &view);
                // Read back, so the frame is finished before the
                // next; the copy of 33 MB is in the figure.
                let _ = renderer.read_back(&target);
            }
            start.elapsed().as_secs_f64() * 1e3 / f64::from(frames)
        };
        time(3);
        eprintln!(
            "{curve:?}: {:.2} ms a 4K frame, read back included",
            time(20)
        );
    }
}

/// The auto white balance's grid is the picture's own pixels at
/// the grid's points, as the CPU would pick them, row and column.
#[test]
fn the_sample_grid_reads_the_pixels_on_its_points() {
    let Some((device, queue)) = device("the sample grid's check") else {
        return;
    };
    let image = field();
    let (w, h) = (image.width, image.height);
    let mut renderer = Renderer::new(&device, &queue);
    assert!(renderer.sample_grid(64).unwrap().is_none());
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    let (grid, width) = renderer.sample_grid(64).unwrap().unwrap();
    let step = w / 64;
    assert_eq!(width, (step / 2..w).step_by(step).count());
    let mut want = Vec::new();
    for y in (step / 2..h).step_by(step) {
        for x in (step / 2..w).step_by(step) {
            let i = (y * w + x) * 3;
            want.push([0, 1, 2].map(|k| half::f16::from_f32(image.data[i + k]).to_f32()));
        }
    }
    assert_eq!(grid.len(), want.len());
    assert_eq!(grid, want);
}

/// A picture already rendered for a display takes a clip where a
/// raw takes the curve, on the GPU as `finish_pixel` has it, so the
/// source's mapping to the shader's mode is checked both ways.
#[test]
fn a_rendered_picture_takes_a_clip_on_the_gpu_as_on_the_cpu() {
    let Some((device, queue)) = device("the rendered picture's check") else {
        return;
    };
    let image = field();
    let (w, h) = (image.width, image.height);
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    let seen: Vec<f32> = image
        .data
        .iter()
        .map(|v| half::f16::from_f32(*v).to_f32())
        .collect();
    let to_out = crate::export::Space::Srgb.matrix();
    let mut worst_of = |source: Source| {
        let mut view = View::blank();
        view.center = (w as f32 / 2.0, h as f32 / 2.0);
        view.plane = (w as f32, h as f32);
        view.frame_size = (w as f32, h as f32);
        view.source = source;
        let target = renderer.render(w as u32, h as u32, &view).0;
        let shown = renderer.read_back(&target).expect("read back");
        let global = Baked::global(&greycard_edit::Edit::default(), source);
        let mut worst = 0.0f32;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                let px = [seen[i], seen[i + 1], seen[i + 2]];
                let cpu = crate::finish::finish_pixel(px, &global, &[], &to_out);
                let gpu = shown.get_pixel(x as u32, y as u32);
                for k in 0..3 {
                    worst = worst.max((cpu[k] - f32::from(gpu[k]) / 255.0).abs());
                }
            }
        }
        worst
    };
    let display = worst_of(Source::Display);
    let scene = worst_of(Source::SCENE);
    eprintln!("GPU against CPU: rendered {display:.4}, raw {scene:.4}");
    assert!(display < 2.5 / 255.0, "{display}");
    assert!(scene < 2.5 / 255.0, "{scene}");
}

/// The same on a real frame, developed at the defaults, with the
/// CPU's time for the masks over it: `GREYCARD_MASK_FRAME` names
/// the raw. Run it in release, ignored tests included, with the
/// output shown.
#[test]
#[ignore]
fn a_real_frames_range_masks() {
    use crate::finish::{local_ab, sample};
    let Some(path) = std::env::var_os("GREYCARD_MASK_FRAME") else {
        panic!("set GREYCARD_MASK_FRAME to a raw");
    };
    let frame = greycard_core::decode::decode_path(&path).expect("decode");
    let developed = greycard_core::develop::develop(
        &frame,
        &greycard_core::develop::DevelopSettings::default(),
    )
    .expect("develop");
    let image = developed.image;
    let (w, h) = (image.width, image.height);
    eprintln!("{w} x {h}, {:.1} MP", (w * h) as f32 / 1e6);
    let locals = range_locals();
    // The CPU's cost: the mean a and b once, then each pixel's
    // sample and every mask's weight, as `finish_with` pays it.
    let start = std::time::Instant::now();
    let ab = local_ab(&image);
    let mean_took = start.elapsed();
    let weights: f32 = (0..h)
        .into_par_iter()
        .map(|y| {
            let mut acc = 0.0;
            for x in 0..w {
                let i = y * w + x;
                let px = image.pixel(x, y);
                let s = Some(sample(px, Some(ab[i]), 0.0));
                let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32);
                for l in &locals {
                    acc += l.weight_sampled(u, v, s);
                }
            }
            acc
        })
        .sum();
    eprintln!(
        "CPU: the mean {:.0} ms, then the sample and {} masks a pixel {:.0} ms (sum {weights:.0})",
        mean_took.as_secs_f64() * 1e3,
        locals.len(),
        (start.elapsed() - mean_took).as_secs_f64() * 1e3
    );
    // And what an export's finish pays whole, the least of five:
    // with no local, a luminance window alone (no mean), a color
    // window.
    let global = Baked::global(&greycard_edit::Edit::default(), Source::SCENE);
    let to_out = crate::export::Space::Srgb.matrix();
    let once = |locals: &[Local]| {
        let start = std::time::Instant::now();
        let out: Vec<u8> = crate::finish::finish_with(
            &image,
            None,
            &global,
            locals,
            |x, y| ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32),
            |_, _| (0.0, None),
            None,
            None,
            None,
            &to_out,
            |v| (v * 255.0).round() as u8,
        );
        std::hint::black_box(out);
        start.elapsed().as_secs_f64() * 1e3
    };
    let finish = |locals: &[Local]| (0..5).map(|_| once(locals)).fold(f64::MAX, f64::min);
    eprintln!(
        "finish: no local {:.0} ms, a luminance window {:.0} ms, a color window {:.0} ms",
        finish(&[]),
        finish(&locals[..1]),
        finish(&locals[1..2])
    );
    let Some((device, queue)) = device("the real frame's GPU check") else {
        return;
    };
    let (worst, mean) = gpu_against_cpu(&device, &queue, &image, &locals, 0.0, true);
    eprintln!("real frame, GPU against CPU: max {worst:.4}, mean {mean:.5}");
    // A few dozen pixels on the whole frame are a few levels out,
    // all in the luminance window's steep high fade, where a small
    // difference in what the shader reads for a pixel is multiplied
    // by the fade's slope; the same pixels cut out on a texture of
    // their own agree to half a level. Which difference in the
    // read that is has not been pinned down.
    assert!(worst <= 4.0 / 255.0, "max {worst}");
    let (cw, ch) = (1024.min(w), 1024.min(h));
    let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
    let mut crop = WorkingImage::new(cw, ch);
    for y in 0..ch {
        let from = ((y0 + y) * w + x0) * 3;
        crop.data[y * cw * 3..(y + 1) * cw * 3].copy_from_slice(&image.data[from..from + cw * 3]);
    }
    let (worst, mean) = gpu_against_cpu(&device, &queue, &crop, &locals, 0.0, false);
    eprintln!("its middle, GPU against CPU: max {worst:.4}, mean {mean:.5}");
    assert!(worst <= 1.5 / 255.0, "max {worst}");
}

/// A small seeded generator for the parity test: splitmix64, each
/// edit seeded from the run's seed and its own index, so one edit
/// is replayed from those two numbers alone.
struct Rng(u64);

impl Rng {
    fn new(seed: u64, k: u64) -> Self {
        let mut r = Rng(seed ^ k.wrapping_mul(0xD1B5_4A32_D192_ED03));
        r.next();
        r
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0 to 1, 24 bits of it.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// A slider: moved, with chance `p`, else at `rest`; moved, at
    /// `lo` a fifth of the time, at `hi` a fifth, and anywhere
    /// between otherwise, since the ends and their sums are where
    /// the arithmetic breaks first. Both draws are taken either
    /// way, so what follows is the same whatever this one says.
    fn slider(&mut self, p: f32, lo: f32, hi: f32, rest: f32) -> f32 {
        let moved = self.chance(p);
        let u = self.unit();
        let v = if u < 0.2 {
            lo
        } else if u >= 0.8 {
            hi
        } else {
            lo + (hi - lo) * (u - 0.2) / 0.6
        };
        if moved { v } else { rest }
    }

    /// A slider always moved: `slider` with its ends as likely.
    fn moved(&mut self, lo: f32, hi: f32) -> f32 {
        self.slider(1.0, lo, hi, lo)
    }
}

/// The parts of an edit the parity test draws, a bit each, so a
/// failing edit can be drawn again with one part left at its
/// default to see which part the difference is in.
const PARTS: [&str; 20] = [
    "light",
    "light switch",
    "point curves",
    "parametric curve",
    "color curves",
    "grading",
    "mixer",
    "color",
    "black and white",
    "tint",
    "vignette",
    "grain",
    "locals",
    "look table",
    "guide plane",
    "display source",
    "output space",
    "display curve",
    "local white",
    "local detail",
];

fn part(name: &str) -> u32 {
    1 << PARTS.iter().position(|p| *p == name).expect("a part")
}

/// One random edit: the edit, and what the viewport and the
/// export are handed beside it.
struct Case {
    edit: greycard_edit::Edit,
    /// The rasters of each adjustment's components, by index.
    rasters: Vec<Vec<Option<RasterRef>>>,
    look: Option<lut::Look>,
    source: Source,
    /// Which guide plane, if any: 0 the picture's own, 1 a coarser
    /// one (the test's `guides`).
    guide: Option<usize>,
    space: crate::export::Space,
    /// The detail planes are handed to both sides (the test's
    /// `planes`), for the masks' Detail to read.
    planes: bool,
}

/// A point curve: the ends at x 0 and 1, up to three points
/// between them from `first` to 0.99 and at least `MIN_GAP` apart,
/// every y anywhere from 0 to 1, as the curve editor lets them be
/// dragged (`panel::curve`). A color curve's ends rest at 0.5, a
/// point curve's on the diagonal.
fn random_points(rng: &mut Rng, color: bool) -> Vec<greycard_edit::curve::Point> {
    use greycard_edit::curve::MIN_GAP;
    let n = rng.below(4);
    // A color curve's points start a twentieth of the way up, not
    // at the panel's 0.01: excluded. The color curves look their
    // shift up by Oklab lightness, a cube root, whose slope at zero
    // has no bound, so near black a point within a few hundredths
    // of the start turns a few billionths of light into a cast, and
    // the shader's fused multiply-adds in the point curves leave
    // such light where the CPU's separate roundings leave none
    // (seed 10, edit 117: 3 levels after a look table). That is the
    // export's own sensitivity, there on any device.
    let first = if color { 0.05 } else { 0.01 };
    let mut xs: Vec<f32> = (0..n).map(|_| rng.moved(first, 0.99)).collect();
    xs.sort_by(f32::total_cmp);
    let mut kept: Vec<f32> = Vec::new();
    for x in xs {
        if kept.last().is_none_or(|l| x - l >= MIN_GAP) {
            kept.push(x);
        }
    }
    let (rest0, rest1) = if color { (0.5, 0.5) } else { (0.0, 1.0) };
    let (y0, y1) = (
        rng.slider(0.4, 0.0, 1.0, rest0),
        rng.slider(0.4, 0.0, 1.0, rest1),
    );
    let mut points = vec![[0.0, y0]];
    points.extend(kept.into_iter().map(|x| [x, rng.moved(0.0, 1.0)]));
    points.push([1.0, y1]);
    points
}

/// A look, every slider over the range the panel gives it
/// (`edit.slint`, `color.slint`, `curve.slint`), each moved with
/// chance `p`; the parts in `skip` left at their defaults.
fn random_look(rng: &mut Rng, p: f32, skip: u32) -> Look {
    use greycard_edit::grading::Wheel;
    let has = |name: &str| skip & part(name) == 0;
    let mut look = Look::default();
    // Light: exposure ±5, contrast 0.5 to 2, highlights, shadows
    // and whites ±2, blacks ±0.3; the section's switch.
    let mut light = Light {
        enabled: !rng.chance(0.15),
        exposure: rng.slider(p, -5.0, 5.0, 0.0),
        ..Light::default()
    };
    light.tone.contrast = rng.slider(p, 0.5, 2.0, 1.0);
    light.tone.highlights = rng.slider(p, -2.0, 2.0, 0.0);
    light.tone.shadows = rng.slider(p, -2.0, 2.0, 0.0);
    light.tone.whites = rng.slider(p, -2.0, 2.0, 0.0);
    light.tone.blacks = rng.slider(p, -0.3, 0.3, 0.0);
    if !has("light switch") {
        light.enabled = true;
    }
    if has("light") {
        look.light = light;
    }
    // The curves: the section's switch, the four channels' points,
    // the parametric's four amounts ±1 and its splits, the two
    // color curves.
    let mut curves = greycard_edit::Curves {
        enabled: !rng.chance(0.1),
        ..Default::default()
    };
    let channels = [
        &mut curves.rgb,
        &mut curves.red,
        &mut curves.green,
        &mut curves.blue,
    ];
    let mut drawn = Vec::new();
    for _ in 0..4 {
        let moved = rng.chance(p);
        let points = random_points(rng, false);
        drawn.push(moved.then_some(points));
    }
    if has("point curves") {
        for (c, d) in channels.into_iter().zip(drawn) {
            if let Some(points) = d {
                *c = points;
            }
        }
    }
    let mut para = greycard_edit::Parametric {
        highlights: rng.slider(p, -1.0, 1.0, 0.0),
        lights: rng.slider(p, -1.0, 1.0, 0.0),
        darks: rng.slider(p, -1.0, 1.0, 0.0),
        shadows: rng.slider(p, -1.0, 1.0, 0.0),
        ..Default::default()
    };
    for i in 0..3 {
        let moved = rng.chance(p);
        let x = rng.unit();
        if moved {
            para.set_split(i, x);
        }
    }
    if has("parametric curve") {
        curves.parametric = para;
    }
    let rg = rng.chance(p).then(|| random_points(rng, true));
    let by = rng.chance(p).then(|| random_points(rng, true));
    if has("color curves") {
        if let Some(points) = rg {
            curves.red_green = points;
        }
        if let Some(points) = by {
            curves.blue_yellow = points;
        }
    }
    look.curves = curves;
    // Grading: three wheels, hue 0 to 360 and strength 0 to 1, and
    // the balance ±1.
    let wheel = |rng: &mut Rng| {
        let moved = rng.chance(p);
        let w = Wheel {
            hue: rng.range(0.0, 360.0),
            saturation: rng.moved(0.0, 1.0),
        };
        if moved { w } else { Wheel::default() }
    };
    let grading = greycard_edit::Grading {
        enabled: !rng.chance(0.1),
        shadows: wheel(rng),
        midtones: wheel(rng),
        highlights: wheel(rng),
        balance: rng.slider(p, -1.0, 1.0, 0.0),
    };
    if has("grading") {
        look.grading = grading;
    }
    // The mixer: eight bands, hue ±30 degrees, saturation and
    // luminance ±1.
    let mut mixer = Mixer {
        enabled: !rng.chance(0.15),
        ..Mixer::default()
    };
    for b in 0..greycard_edit::mixer::BANDS {
        mixer.hue[b] = rng.slider(p * 0.6, -30.0, 30.0, 0.0);
        mixer.saturation[b] = rng.slider(p * 0.6, -1.0, 1.0, 0.0);
        mixer.luminance[b] = rng.slider(p * 0.6, -1.0, 1.0, 0.0);
    }
    if has("mixer") {
        look.mixer = mixer;
    }
    // Color: vibrance and saturation ±1.
    let color = Color {
        enabled: !rng.chance(0.15),
        saturation: rng.slider(p, -1.0, 1.0, 0.0),
        vibrance: rng.slider(p, -1.0, 1.0, 0.0),
    };
    if has("color") {
        look.color = color;
    }
    // The tint: its switch, hue 0 to 360, amount 0 to 1.
    let tint = Tint {
        enabled: !rng.chance(0.15),
        ..Tint::new(rng.range(0.0, 360.0), rng.slider(p, 0.0, 1.0, 0.0))
    };
    if has("tint") {
        look.tint = tint;
    }
    look
}

/// A brush's strokes, a few of them, adds and subtracts and an
/// erase. The radius is drawn from 0.02 to 0.15 of the width
/// rather than the panel's 0.002 to 0.5: painting a raster
/// 2048 texels wide in a debug build costs by the disc's area,
/// and what is checked is the raster's sampling, the same grid
/// of texels whatever the strokes were.
fn random_strokes(rng: &mut Rng, aspect: f32) -> Vec<greycard_edit::brush::Stroke> {
    use greycard_edit::brush::{Op, Stroke};
    let n = 1 + rng.below(3);
    (0..n)
        .map(|i| {
            let op = if i == 0 {
                Op::Add
            } else {
                Op::ALL[rng.below(Op::ALL.len())]
            };
            let mut s = Stroke::new(op, rng.range(0.02, 0.15), rng.unit(), rng.unit());
            let points = 2 + rng.below(3);
            s.points = (0..points)
                .map(|_| [rng.range(-0.1, 1.1), rng.range(-0.1, aspect + 0.1)])
                .collect();
            s
        })
        .collect()
}

/// The rasters the cases draw their raster shapes from, made once:
/// three brushes and a learned mask (a model's, here a pattern of
/// soft blobs and hard edges), each a raster of the picture's
/// aspect, all `RASTER_WIDTH` wide as the app's are.
fn raster_pool(seed: u64, aspect: f32) -> (Vec<(Shape, RasterRef)>, RasterRef) {
    use greycard_edit::brush::Raster;
    let mut rng = Rng::new(seed, u64::MAX);
    let brushes = (0..3)
        .map(|_| {
            let strokes = random_strokes(&mut rng, aspect);
            let raster = Raster::of(&strokes, aspect);
            (Shape::Brush { strokes }, RasterRef(Arc::new(raster)))
        })
        .collect();
    let w = RASTER_WIDTH;
    let h = ((w as f32 * aspect).round() as usize).max(1);
    let data = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let (u, v) = (x as f32 / w as f32, y as f32 / w as f32);
            let blob = (0.5 + 0.5 * (u * 9.0).sin() * (v * 13.0).cos()).clamp(0.0, 1.0);
            let edge = if (u - 0.6).abs() < 0.15 && v > 0.3 {
                1.0
            } else {
                0.0
            };
            ((blob * 0.7 + edge * 0.3) * 255.0).round() as u8
        })
        .collect();
    let learned = RasterRef(Arc::new(Raster::from_data(aspect, w, data)));
    (brushes, learned)
}

/// A mask of one to three shapes, each of any kind in any mode,
/// on, off or inverted, with the raster each needs.
fn random_mask(
    rng: &mut Rng,
    aspect: f32,
    brushes: &[(Shape, RasterRef)],
    learned: &RasterRef,
) -> (Mask, Vec<Option<RasterRef>>) {
    let n = 1 + rng.below(3);
    let mut components = Vec::new();
    let mut rasters = Vec::new();
    for _ in 0..n {
        let (shape, raster) = match rng.below(7) {
            0 => (
                Shape::Linear {
                    from: [rng.range(-0.2, 1.2), rng.range(-0.2, aspect + 0.2)],
                    to: [rng.range(-0.2, 1.2), rng.range(-0.2, aspect + 0.2)],
                },
                None,
            ),
            1 => (
                Shape::Radial {
                    center: [rng.range(0.0, 1.0), rng.range(0.0, aspect)],
                    radius: [rng.range(0.02, 0.7), rng.range(0.02, 0.7)],
                    angle: rng.range(0.0, 360.0),
                    feather: rng.moved(0.0, 1.0),
                },
                None,
            ),
            2 => {
                let (shape, raster) = &brushes[rng.below(brushes.len())];
                (shape.clone(), Some(raster.clone()))
            }
            3 => {
                // A learned shape, its raster there or, now and
                // then, still on its way.
                let there = !rng.chance(0.2);
                (Shape::Subject {}, there.then(|| learned.clone()))
            }
            // The panel's ranges: low and high 0 to 1, their
            // fades 0 to 0.5 (`mask.slint`).
            4 => (
                Shape::Luminance {
                    low: rng.slider(0.8, 0.0, 1.0, 0.0),
                    high: rng.slider(0.8, 0.0, 1.0, 1.0),
                    low_feather: rng.moved(0.0, 0.5),
                    high_feather: rng.moved(0.0, 0.5),
                },
                None,
            ),
            // Hue 0 to 360, width 0 to 180, its fade 0 to 90,
            // chroma 0 to 0.2, its fade 0 to 0.1.
            _ => (
                Shape::Color {
                    hue: rng.range(0.0, 360.0),
                    width: rng.moved(0.0, 180.0),
                    hue_feather: rng.moved(0.0, 90.0),
                    chroma: rng.moved(0.0, 0.2),
                    chroma_feather: rng.moved(0.0, 0.1),
                },
                None,
            ),
        };
        components.push(Component {
            shape,
            mode: Mode::ALL[rng.below(Mode::ALL.len())],
            invert: rng.chance(0.2),
            enabled: !rng.chance(0.1),
        });
        rasters.push(raster);
    }
    let mask = Mask {
        components,
        invert: rng.chance(0.2),
    };
    (mask, rasters)
}

/// Three look tables with a cross term, one channel's output
/// moved by the other two's product, so a lookup that read the
/// wrong corner of a cell would show: an sRGB one at 9 nodes, a
/// linear Rec.2020 one at 5 with a domain past one, and a gamma
/// 2.2 Display P3 one at 17.
fn look_tables() -> Vec<Arc<lut::Lut3d>> {
    let table = |size: usize,
                 encoding: lut::Encoding,
                 primaries: lut::Primaries,
                 max: f32,
                 f: &dyn Fn([f32; 3]) -> [f32; 3]| {
        let mut t = lut::Lut3d::identity(size);
        t.encoding = encoding;
        t.primaries = primaries;
        t.domain_max = [max; 3];
        let last = (size - 1) as f32;
        let mut i = 0;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let c = [r, g, b].map(|v| v as f32 / last * max);
                    t.data[i] = f(c);
                    i += 1;
                }
            }
        }
        for px in &mut t.data {
            *px = px.map(|v| half::f16::from_f32(v).to_f32());
        }
        Arc::new(t)
    };
    vec![
        table(
            9,
            lut::Encoding::Srgb,
            lut::Primaries::Srgb,
            1.0,
            &|[r, g, b]| {
                [
                    0.9 * r + 0.1 * g * b,
                    g * g * 0.8 + 0.2 * r,
                    (b + 0.3 * r * g).min(1.2),
                ]
            },
        ),
        table(
            5,
            lut::Encoding::Linear,
            lut::Primaries::Rec2020,
            1.25,
            &|[r, g, b]| [r * 0.8 + 0.2 * g * b, g, b * 0.9 + 0.1 * r],
        ),
        table(
            17,
            lut::Encoding::Gamma(2.2),
            lut::Primaries::DisplayP3,
            1.0,
            &|[r, g, b]| {
                let m = (r + g + b) / 3.0;
                [
                    m + 1.2 * (r - m) + 0.05 * g * b,
                    m + 0.9 * (g - m),
                    b.sqrt(),
                ]
            },
        ),
    ]
}

/// Edit `k` of the run seeded `seed`, with the parts in `skip` at
/// their defaults.
fn random_case(
    seed: u64,
    k: u64,
    skip: u32,
    aspect: f32,
    pool: &(Vec<(Shape, RasterRef)>, RasterRef),
    tables: &[Arc<lut::Lut3d>],
) -> Case {
    use greycard_edit::grain::Kind;
    let has = |name: &str| skip & part(name) == 0;
    let mut rng = Rng::new(seed, k);
    let mut edit = greycard_edit::Edit::default();
    edit.set_look(random_look(&mut rng, 0.5, skip));
    // The black and white: eight weights ±1 and a strength 0 to 3.
    let mut bw = BlackWhite {
        enabled: rng.chance(0.3),
        weights: [0.0; 8],
        strength: rng.moved(0.0, greycard_edit::bw::STRENGTH_MAX),
    };
    for b in 0..8 {
        bw.weights[b] = rng.slider(0.5, -1.0, 1.0, 0.0);
    }
    if has("black and white") {
        edit.bw = bw;
    }
    // The vignette: amount ±5 stops, midpoint and feather 0 to 1,
    // roundness ±1.
    let vignette = Vignette {
        enabled: !rng.chance(0.1),
        amount: rng.slider(0.5, -5.0, 5.0, 0.0),
        midpoint: rng.moved(0.0, 1.0),
        feather: rng.moved(0.0, 1.0),
        roundness: rng.moved(-1.0, 1.0),
    };
    if has("vignette") {
        edit.vignette = vignette;
    }
    // The grain: amount 0 to 1, size 0.1 to 2, either kind.
    let grain = Grain {
        enabled: !rng.chance(0.1),
        amount: rng.slider(0.4, 0.0, 1.0, 0.0),
        size: rng.moved(0.1, 2.0),
        kind: Kind::ALL[rng.below(Kind::ALL.len())],
    };
    if has("grain") {
        edit.grain = grain;
    }
    // Two or three local adjustments, each its own look.
    let n = 2 + rng.below(2);
    let mut rasters = Vec::new();
    for id in 0..n {
        let (mask, r) = random_mask(&mut rng, aspect, &pool.0, &pool.1);
        let look = random_look(&mut rng, 0.35, skip);
        let enabled = !rng.chance(0.1);
        if has("locals") {
            edit.adjustments.push(greycard_edit::Adjustment {
                id: id as u64,
                enabled,
                mask,
                look,
                ..Default::default()
            });
            rasters.push(r);
        }
    }
    // A look table, at any strength.
    let which = rng.below(tables.len() + 1);
    let strength = rng.moved(0.0, 1.0);
    let look = (has("look table") && which < tables.len())
        .then(|| lut::Look::new(tables[which].clone(), strength).expect("a look"));
    let display = rng.chance(0.25);
    let guide = rng.chance(0.6);
    let coarse = rng.chance(0.5);
    let space = crate::export::Space::ALL[rng.below(crate::export::Space::ALL.len())];
    // AgX half the time: drawn last, so the draws before it are
    // the ones every seed had before.
    if rng.chance(0.5) && has("display curve") {
        edit.display_curve = greycard_edit::DisplayCurve::Agx;
    }
    // Each mask's own white balance half the time, over the
    // panel's ranges: after AgX, for the same reason.
    for a in &mut edit.adjustments {
        let on = rng.chance(0.5);
        let temperature = rng.range(2000.0, 12000.0) as f64;
        let tint = rng.range(-0.05, 0.05) as f64;
        if on && has("local white") {
            a.look.white_balance = greycard_edit::LocalWhite::Absolute { temperature, tint };
        }
    }
    // Each mask's Detail, over the panel's ranges, its switch off
    // now and then, and the planes there most of the time: after
    // the local white, for the same reason.
    for a in &mut edit.adjustments {
        let detail = greycard_edit::Detail {
            enabled: !rng.chance(0.15),
            texture: rng.slider(0.5, -1.0, 1.0, 0.0),
            clarity: rng.slider(0.5, -1.0, 1.0, 0.0),
            dehaze: rng.slider(0.5, -1.0, 1.0, 0.0),
        };
        if has("local detail") {
            a.look.detail = detail;
        }
    }
    let planes = !rng.chance(0.2) && has("local detail");
    Case {
        edit,
        rasters,
        look,
        source: if display && has("display source") {
            Source::Display
        } else {
            Source::SCENE
        },
        guide: (guide && has("guide plane")).then_some(usize::from(coarse)),
        space: if has("output space") {
            space
        } else {
            crate::export::Space::Srgb
        },
        planes,
    }
}

/// Detail planes for the parity test: bands of a few stops either
/// way and a haze from nothing to most of the way, varying pixel to
/// pixel, so a plane read at the wrong texel shows; the airlight
/// warm, and the clip at one, where the frame has highlights past.
fn parity_planes(w: usize, h: usize) -> crate::detail::DetailPlanes {
    let mut rng = Rng::new(7, u64::MAX - 1);
    let data = (0..w * h)
        .flat_map(|_| {
            [
                rng.range(-1.5, 1.5),
                rng.range(-1.0, 1.0),
                rng.range(0.0, 0.9),
                0.0,
            ]
        })
        .map(half::f16::from_f32)
        .collect();
    crate::detail::DetailPlanes {
        width: w,
        height: h,
        data,
        meta: crate::detail::DetailMeta {
            airlight: [0.9, 0.86, 0.8],
            clip_level: 1.0,
        },
        wants: greycard_edit::DetailWants {
            texture: true,
            clarity: true,
            haze: true,
        },
    }
}

/// A small frame with the corners of what a picture holds: rows of
/// near black, an Oklab field from grey to chroma 0.3 at every
/// hue, the working space's primaries alone at a range of levels
/// (the other two channels exactly zero), and highlights past
/// white, some with one channel clipped high; a little noise over
/// all of it so the local mean has something to do.
fn parity_frame() -> WorkingImage {
    let (w, h) = (48usize, 32usize);
    let ok = greycard_core::color::Oklab::for_working_space();
    let from_lms = greycard_core::color::invert3(ok.to_lms).unwrap();
    let hash = |x: usize, y: usize, c: usize| {
        let mut z = (x as u64) << 32 | (y as u64) << 8 | c as u64;
        z = (z ^ (z >> 33)).wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        z = (z ^ (z >> 33)).wrapping_mul(0xC4CE_B9FE_1A85_EC53);
        ((z ^ (z >> 33)) >> 40) as f32 / (1u64 << 24) as f32
    };
    let lab_to = |lab: [f32; 3]| {
        let lms =
            greycard_core::color::apply3(&greycard_core::color::LAB_TO_LMS, lab).map(|v| v * v * v);
        greycard_core::color::apply3(&from_lms, lms).map(|v| v.max(0.0))
    };
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let n = |c| 1.0 + 0.06 * (hash(x, y, c) - 0.5);
            let px = match y {
                0..=3 => {
                    let level = [0.0, 2e-5, 3e-4, 4e-3][y];
                    [0, 1, 2].map(|c| level * hash(x, y, c) * 2.0)
                }
                4..=21 => {
                    let l = 0.12 + 0.85 * (y - 4) as f32 / 17.0;
                    let hue = (x as f32 * 7.5).to_radians();
                    let chroma = 0.3 * ((x % 4) as f32 / 3.0);
                    let c = lab_to([l, chroma * hue.cos(), chroma * hue.sin()]);
                    [0, 1, 2].map(|k| c[k] * n(k))
                }
                22..=25 => {
                    let level = 0.01 * 1.3f32.powi((x / 3) as i32 + 4 * (y - 22) as i32 % 16);
                    let mut c = [0.0; 3];
                    c[x % 3] = level;
                    c
                }
                _ => {
                    let over = 1.0 + 7.0 * x as f32 / w as f32;
                    let hue = (x as f32 * 31.0).to_radians();
                    let base = lab_to([0.8, 0.08 * hue.cos(), 0.08 * hue.sin()]);
                    let mut c = base.map(|v| v * over);
                    if y % 2 == 0 {
                        c[x % 3] = 6.0;
                    }
                    [0, 1, 2].map(|k| c[k] * n(k))
                }
            };
            data.extend(px);
        }
    }
    WorkingImage {
        width: w,
        height: h,
        data,
    }
}

/// What one edit came to: how far the GPU was at worst from the
/// nearest of the CPU's answers, where, the CPU's answer and the
/// GPU's there, how many pixels were steep (their answers spread
/// wider than `STEEP`), and how many channels' answers spread wider
/// than `WIDE`.
struct Outcome {
    worst: f32,
    at: (usize, usize, usize),
    cpu: [f32; 3],
    gpu: [f32; 3],
    steep: usize,
    /// The steep pixels among the lit ones (`lit`), which
    /// `STEEP_SHARE` holds.
    steep_lit: usize,
    wide: usize,
    /// Channels whose answers the finest nudges alone spread
    /// wider than the tolerance (`FINEST`).
    unsettled: usize,
    /// Channels checked again against the finer nudges (`REFINED`).
    refined: usize,
    /// The CPU's answers in the worst channel, as it is first.
    answers: Vec<f32>,
}

/// How far each channel of the picture is moved, up and then down,
/// as a share of the pixel's largest, for the CPU's answers: the
/// picture as it is and so moved, at each of `NUDGES` of this. The
/// GPU passes where it lies within the tolerance of one of them,
/// not merely between the least and the most, so a spread over the
/// whole scale cannot pass a black. Where the CPU's answer is flat,
/// they are one answer; where an error of the size the GPU's
/// arithmetic is allowed moves it far, they spread with it, and they
/// are graded rather than one step each way because such a pixel's
/// answer is a continuum the GPU can land anywhere on: at one step
/// of 1e-5 each way, five edits over seeds 1 to 10 on Mesa fell
/// between answers 50 to 250 levels apart, and none does graded.
/// What makes pixels steep, all found by this test:
///
/// - A large stacked gain into the white point, whose exponent runs
///   to tens of stops at the sliders' corners with the masks' added:
///   it carries a few ULP of the GPU's `log2` and `exp2` to 1e-5 of
///   the value and past, and a steep point curve, or the output
///   matrix's cancellation into a dark channel where the sRGB curve
///   is steepest, makes that whole levels.
/// - A contrast under one, a power with no bound on its slope at
///   zero, on a channel the Oklab pass leaves near zero.
///
/// The brush rasters and the guide plane are read by texel and
/// interpolated in f32 on both sides (`raster_at`, `guide_at` in
/// the shader), so neither needs a nudge: through the sampler,
/// lavapipe's filtered read of a raster came back a third of a
/// level off, and 45 of the default seed's 400 edits fell outside
/// the bracket.
///
/// A share of the pixel's largest channel, the nudge does not move
/// black; and a window's weight at no light can be 6e-8 on one side
/// and nothing on the other, which the point and color curves near
/// black make whole levels (default seed, edits 1324 and 2565, 35
/// and 12 levels, outside the bracket at 5 000 edits; not at 400).
const NUDGE: f32 = 1e-5;
const NUDGES: [f32; 4] = [1.0, 0.3, 0.1, 0.03];
/// The finest nudges' answers, the last of the run's: the smallest
/// step, both signs, each channel. That step, 3e-7 of the largest
/// channel, is two to five of its roundings; where it alone spreads
/// a channel's answers wider than the tolerance, the CPU's answer
/// there is not settled at f32's precision, and the GPU, whose
/// arithmetic is allowed about as much, is held to the range of
/// all the answers rather than to the nearest one. The answers
/// there are samples of a continuum steep enough that the GPU lands
/// between them: seed 55 edit 121, a channel the Oklab pass leaves
/// at a two-thousandth of the largest, taken by a contrast under one
/// and a point curve rising at zero, answers 57 to 242 levels and
/// lavapipe at 134, 25 levels from the nearest; and three more past
/// seed 10, by 2.7 to 4.5 levels, on one driver or two. A black
/// still fails inside a range that does not reach it, and a pixel
/// whose finest answers agree is held to the nearest as before.
const FINEST: usize = 2 * 3;
/// The most channels of an edit that may be unsettled, and the most
/// an edit may have on average over the run: the range they are
/// held to is a looser check than the nearest answer, nearly all of
/// them are in the frame's empty channels, which the steep cap
/// does not count, and a change that made those answers chaotic in
/// a few edits in a hundred would otherwise pass. Over seeds 1 to
/// 150 the most in one edit is 630 of the frame's 4608 channels
/// (seed 133, edit 81, every steep pixel of it empty), then 425
/// (seed 74, edit 395); the most in a run is 631, 1.6 an edit. The
/// caps are a half and three times over those.
const UNSETTLED_MAX: usize = 1000;
const UNSETTLED_MEAN: f64 = 5.0;
/// How many steps the finer nudges take from `NUDGE` down to the
/// finest, evenly in their logarithm, each channel alone and all
/// three together, both signs: run for an edit only where a channel
/// of the GPU's lies between two of the answers and within the
/// tolerance of none, which a continuous answer steep at the scale
/// of the nudges can do (seed 129 edit 102 on lavapipe: a channel
/// the Oklab pass takes from nothing to 0.055, which the blacks'
/// crush at 0.054 leaves at 9e-4 and a steep point curve lifts,
/// answers 186 to 255 levels and the GPU 7.9 from the nearest of
/// them). A value off every answer of the finer sampling still
/// fails, so an answer that jumps cannot pass between its sides.
const REFINED: usize = 64;
/// How far an encoded channel may be from the CPU's answers.
const TOLERANCE: f32 = 2.5 / 255.0;
/// The spread of a channel's answers past which it counts as wide,
/// a few of which every run has: the run prints how many.
const WIDE: f32 = 32.0 / 255.0;
/// The bracket's width from which a pixel counts as steep: wider
/// than the tolerance, so that the bracket loosens the check by
/// more than the tolerance itself does. A narrower width, a level,
/// put a few edits at the sliders' corners over `STEEP_SHARE`.
const STEEP: f32 = 2.5 / 255.0;
/// The most of an edit's lit pixels (`lit` in the test: every
/// channel holds more than the nudge puts in) that may be steep:
/// past it the bracket is checking too little of the picture for
/// the edit to count, and it fails. The frame's other pixels, 461
/// of its 1536 besides the black row (the rows of lone primaries
/// and the Oklab field's colors clipped at a channel), are steep
/// wherever the edit has a curve steep at zero, as a point at the
/// panel's 0.01 rising makes it: counted with them, the steepest
/// edit of seeds 1 to 10 was 11.1% and the cap of 15% set from it
/// failed five edits in 40 000 past seed 10 (seed 74 edit 395 at
/// 21.4%, and every steep pixel of the five with an empty channel,
/// none lit; the same on every device, since it is the CPU's own
/// spread). The lit share's steepest over seeds 1 to 150 is 0.78%
/// (seed 63, edit 133); a break that made answers jump shows in
/// many times that. `STEEP_MEAN` holds the run as a whole.
const STEEP_SHARE: f64 = 0.03;
/// The most of the pixels that may be steep over the whole run, so
/// that a change that made every answer jump cannot pass by
/// widening every bracket a little.
const STEEP_MEAN: f64 = 0.01;

/// A mask over everything (a luminance window from 0 to 1) with
/// `look` under it.
fn everywhere(look: Look) -> greycard_edit::Adjustment {
    greycard_edit::Adjustment {
        mask: Mask {
            components: vec![Component {
                shape: Shape::Luminance {
                    low: 0.0,
                    high: 1.0,
                    low_feather: 0.0,
                    high_feather: 0.0,
                },
                ..Default::default()
            }],
            invert: false,
        },
        look,
        ..Default::default()
    }
}

/// `edit` over `image` through both sides, sRGB, eight bits: the
/// CPU's `finish_with` as the export calls it and the shader's
/// picture read back.
fn both_sides(
    device: &gpu::Device,
    queue: &gpu::Queue,
    image: &WorkingImage,
    edit: &greycard_edit::Edit,
) -> (Vec<u8>, Vec<u8>) {
    both_sides_with(device, queue, image, edit, None)
}

/// [`both_sides`] with the masks' detail planes handed to each.
fn both_sides_with(
    device: &gpu::Device,
    queue: &gpu::Queue,
    image: &WorkingImage,
    edit: &greycard_edit::Edit,
    planes: Option<&Arc<crate::detail::DetailPlanes>>,
) -> (Vec<u8>, Vec<u8>) {
    let (w, h) = (image.width, image.height);
    let shift = crate::testing::white_shift();
    let locals: Vec<Local> = edit
        .adjustments
        .iter()
        .map(|a| Local::of(a, vec![None; a.mask.components.len()], Some(&shift)))
        .collect();
    let mut renderer = Renderer::new(device, queue);
    renderer.upload(&crate::worker::Halves::from_image(image, None));
    renderer.set_detail(planes);
    let bands = planes.map(|p| p.to_image());
    let view = View {
        center: (w as f32 / 2.0, h as f32 / 2.0),
        plane: (w as f32, h as f32),
        frame_size: (w as f32, h as f32),
        locals: locals.clone(),
        ..View::with_look(edit)
    };
    let target = renderer.render(w as u32, h as u32, &view).0;
    let shown = renderer.read_back(&target).expect("read back");
    let gpu = shown.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let (iw, ih) = (w as f32, h as f32);
    let vignette = edit.vignette;
    let cpu = crate::finish::finish_with(
        image,
        None,
        &Baked::global(edit, Source::SCENE),
        &locals,
        |x, y| ((x as f32 + 0.5) / iw, (y as f32 + 0.5) / iw),
        |x, y| {
            let (u, v) = ((x as f32 + 0.5) / iw, (y as f32 + 0.5) / ih);
            let stops = if vignette.is_off() {
                0.0
            } else {
                vignette.amount * vignette.at(u, v, iw / ih)
            };
            (stops, None)
        },
        None,
        bands.as_ref().zip(planes.map(|p| &p.meta)),
        None,
        &crate::export::Space::Srgb.matrix(),
        |v| (v * 255.0).round() as u8,
    );
    (cpu, gpu)
}

/// A mask's Detail timed on the GPU at 24 and 45 MP: the planes'
/// upload, and a 2560 by 1440 draw of the whole frame with and
/// without a mask asking, each read back so the time is the
/// device's. A measurement, so ignored.
#[test]
#[ignore = "a measurement"]
fn a_masks_detail_timed_on_the_gpu() {
    let Some((device, queue)) = device("the masks' Detail timing") else {
        return;
    };
    for (w, h) in [(6000usize, 4000usize), (8192, 5464)] {
        let image = WorkingImage {
            width: w,
            height: h,
            data: (0..w * h * 3)
                .map(|i| 0.05 + (i % 97) as f32 / 200.0)
                .collect(),
        };
        let planes = Arc::new(crate::detail::DetailPlanes {
            width: w,
            height: h,
            data: (0..w * h * 4)
                .map(|i| half::f16::from_f32((i % 13) as f32 / 20.0 - 0.3))
                .collect(),
            meta: crate::detail::DetailMeta {
                airlight: [0.9, 0.86, 0.8],
                clip_level: 1.0,
            },
            wants: greycard_edit::DetailWants {
                texture: true,
                clarity: true,
                haze: true,
            },
        });
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        let look = Look {
            detail: greycard_edit::Detail {
                enabled: true,
                texture: -0.5,
                clarity: 0.3,
                dehaze: 0.3,
            },
            ..Default::default()
        };
        let mut edit = greycard_edit::Edit::default();
        edit.adjustments.push(everywhere(look));
        let locals: Vec<Local> = edit
            .adjustments
            .iter()
            .map(|a| Local::of(a, vec![None], None))
            .collect();
        let (vw, vh) = (2560u32, 1440u32);
        let view = View {
            center: (w as f32 / 2.0, h as f32 / 2.0),
            plane: (w as f32, h as f32),
            frame_size: (w as f32, h as f32),
            zoom: (vw as f32 / w as f32).min(vh as f32 / h as f32),
            locals,
            ..View::with_look(&edit)
        };
        let draw = |renderer: &mut Renderer| {
            let mut best = f64::INFINITY;
            for _ in 0..5 {
                renderer.redraw = true;
                let started = std::time::Instant::now();
                let target = renderer.render(vw, vh, &view).0;
                renderer.read_back(&target).expect("read back");
                best = best.min(started.elapsed().as_secs_f64());
            }
            best
        };
        let without = draw(&mut renderer);
        let started = std::time::Instant::now();
        renderer.set_detail(Some(&planes));
        let target = renderer.render(1, 1, &view).0;
        renderer.read_back(&target).expect("read back");
        let upload = started.elapsed().as_secs_f64();
        let with = draw(&mut renderer);
        eprintln!(
            "{w}x{h}: the planes up in {:.0} ms; a {vw}x{vh} draw {:.1} ms without them, \
             {:.1} ms with a mask's three",
            upload * 1e3,
            without * 1e3,
            with * 1e3
        );
    }
}

/// A mask's Detail on both sides: Texture, Clarity and Dehaze over
/// the whole frame from the planes of the picture, the shader the
/// CPU's to two levels; nothing without the planes, and nothing
/// with the section off.
#[test]
fn a_masks_detail_on_both_sides() {
    let Some((device, queue)) = device("the masks' Detail check") else {
        return;
    };
    let image = parity_frame();
    let planes = crate::detail::DetailPlanes::make(
        &image,
        None,
        1.0,
        greycard_edit::DetailWants {
            texture: true,
            clarity: true,
            haze: true,
        },
    )
    .map(Arc::new)
    .expect("planes");
    let look = Look {
        detail: greycard_edit::Detail {
            enabled: true,
            texture: 0.8,
            clarity: -0.6,
            dehaze: 0.7,
        },
        ..Default::default()
    };
    let mut edit = greycard_edit::Edit::default();
    edit.adjustments.push(everywhere(look));
    let (cpu, gpu) = both_sides_with(&device, &queue, &image, &edit, Some(&planes));
    let worst = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max();
    assert!(worst <= Some(2), "{worst:?}");
    let (bare, bare_gpu) = both_sides_with(&device, &queue, &image, &edit, None);
    assert!(bare != cpu, "the Detail did nothing");
    let worst = bare
        .iter()
        .zip(&bare_gpu)
        .map(|(a, b)| a.abs_diff(*b))
        .max();
    assert!(worst <= Some(2), "{worst:?}");
    edit.adjustments[0].look.detail.enabled = false;
    let (off, off_gpu) = both_sides_with(&device, &queue, &image, &edit, Some(&planes));
    assert_eq!(off, bare);
    assert_eq!(off_gpu, bare_gpu);
}

/// The stacked corner the random draws found the shader's display
/// curve going black at: global exposure +5, contrast 2 and whites
/// +2, the vignette's +5 at the corners, and a mask over everything
/// at +5 more, on a pixel of 6. The scene comes to around 1e31, past
/// where the fit's squares overflow; both sides make it white.
#[test]
fn the_stacked_corner_is_white_on_both_sides() {
    let Some((device, queue)) = device("the stacked corner's check") else {
        return;
    };
    let (w, h) = (8usize, 6usize);
    let image = WorkingImage {
        width: w,
        height: h,
        data: vec![6.0; w * h * 3],
    };
    let mut edit = greycard_edit::Edit::default();
    edit.light.exposure = 5.0;
    edit.light.tone.contrast = 2.0;
    edit.light.tone.whites = 2.0;
    edit.vignette.amount = 5.0;
    let mut look = Look::default();
    look.light.exposure = 5.0;
    edit.adjustments.push(everywhere(look));
    let (cpu, gpu) = both_sides(&device, &queue, &image, &edit);
    assert!(cpu.iter().all(|&v| v == 255), "the CPU: {cpu:?}");
    assert!(gpu.iter().all(|&v| v == 255), "the GPU: {gpu:?}");
}

/// A mask's own white balance on both sides: a warm one over
/// everything with the mixer moved under it, so the mixer's mean
/// is taken with the pixel (`finish::rebalanced`), and a cool one
/// at part weight over half the frame, blended over the first.
#[test]
fn a_masks_white_balance_agrees_on_both_sides() {
    let Some((device, queue)) = device("the masks' white balance check") else {
        return;
    };
    let (w, h) = (24usize, 8usize);
    let image = WorkingImage {
        width: w,
        height: h,
        data: (0..w * h)
            .flat_map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let v = 0.02 + 0.03 * x;
                [v * (1.0 + 0.1 * y), v * 0.9, v * (1.2 - 0.1 * y)]
            })
            .collect(),
    };
    let mut edit = greycard_edit::Edit::default();
    let mut warm = Look {
        white_balance: greycard_edit::LocalWhite::Absolute {
            temperature: 3200.0,
            tint: 0.004,
        },
        ..Look::default()
    };
    warm.mixer.hue[1] = 20.0;
    warm.mixer.saturation[5] = -0.6;
    edit.adjustments.push(everywhere(warm));
    let cool = Look {
        white_balance: greycard_edit::LocalWhite::Absolute {
            temperature: 9000.0,
            tint: -0.01,
        },
        ..Look::default()
    };
    edit.adjustments.push(greycard_edit::Adjustment {
        id: 2,
        mask: Mask {
            components: vec![Component {
                shape: Shape::Linear {
                    from: [0.3, 0.0],
                    to: [0.7, 0.0],
                },
                ..Default::default()
            }],
            invert: false,
        },
        look: cool,
        ..Default::default()
    });
    let (cpu, gpu) = both_sides(&device, &queue, &image, &edit);
    let worst = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max();
    assert!(worst <= Some(1), "{worst:?}: {cpu:?} against {gpu:?}");
    // And it did something: without the whites the picture differs.
    let mut plain = edit.clone();
    for a in &mut plain.adjustments {
        a.look.white_balance = greycard_edit::LocalWhite::Off;
    }
    let (bare, _) = both_sides(&device, &queue, &image, &plain);
    let moved = cpu.iter().zip(&bare).map(|(a, b)| a.abs_diff(*b)).max();
    assert!(moved >= Some(20), "{moved:?}");
}

/// Point curves with their ends slid in along their edges, on both
/// sides: the global master's black past 0.15 and white past 0.85,
/// red's white past 0.7, and a mask over everything whose blue curve
/// has its black past 0.3. Past an end both sides read the end's Out
/// from the same table. Each slid end is seen to bite: against the
/// same curves with their ends at the corners, more of the picture is
/// black and white, more red is white with red's end slid, and more
/// blue black with the mask's.
#[test]
fn slid_curve_ends_agree_on_both_sides() {
    let Some((device, queue)) = device("the slid curve ends' check") else {
        return;
    };
    let image = parity_frame();
    // The edit with the named curves' ends at the corners.
    let corners = |rgb: bool, red: bool, blue: bool| {
        let mut edit = greycard_edit::Edit::default();
        edit.curves.rgb = if rgb {
            vec![[0.0, 0.0], [0.5, 0.45], [1.0, 1.0]]
        } else {
            vec![[0.15, 0.0], [0.5, 0.45], [0.85, 1.0]]
        };
        edit.curves.red = if red {
            vec![[0.0, 0.0], [1.0, 1.0]]
        } else {
            vec![[0.0, 0.0], [0.7, 1.0]]
        };
        let mut look = Look::default();
        look.curves.blue = if blue {
            vec![[0.0, 0.0], [1.0, 1.0]]
        } else {
            vec![[0.3, 0.0], [1.0, 1.0]]
        };
        edit.adjustments.push(everywhere(look));
        edit
    };
    let (cpu, gpu) = both_sides(&device, &queue, &image, &corners(false, false, false));
    let worst = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max();
    assert!(worst <= Some(1), "{worst:?}");
    // How many of channel `c` (all three for None) are at `v`.
    let count = |px: &[u8], c: Option<usize>, v: u8| {
        px.iter()
            .enumerate()
            .filter(|(i, p)| c.is_none_or(|c| i % 3 == c) && **p == v)
            .count()
    };
    let (cornered, _) = both_sides(&device, &queue, &image, &corners(true, true, true));
    let (red_cornered, _) = both_sides(&device, &queue, &image, &corners(false, true, false));
    let (blue_cornered, _) = both_sides(&device, &queue, &image, &corners(false, false, true));
    let pixels = cpu.len() / 3;
    let more = |slid: usize, at_corners: usize, what: &str| {
        eprintln!("{what}: {slid} slid, {at_corners} at the corners");
        assert!(
            slid > at_corners + pixels / 50,
            "{what}: {slid} against {at_corners}"
        );
    };
    more(count(&cpu, None, 0), count(&cornered, None, 0), "black");
    more(count(&cpu, None, 255), count(&cornered, None, 255), "white");
    more(
        count(&cpu, Some(0), 255),
        count(&red_cornered, Some(0), 255),
        "red's white",
    );
    more(
        count(&cpu, Some(2), 0),
        count(&blue_cornered, Some(2), 0),
        "the mask's blue black",
    );
}

/// A mask's own white balance while the global's slider is moving:
/// the view previews 6500 K over a develop at 5500 K, and a mask
/// over everything names 3200 K with the mixer moved under it. The
/// CPU is handed what the develop at 6500 K would be (the picture
/// through the preview's matrix) and the mask's matrix relative to
/// that (`Mᵢ · W⁻¹`), which is what the export makes once the
/// develop lands. The mixer's mean, read through the preview on the
/// GPU, must not take the preview twice.
#[test]
fn a_masks_white_balance_holds_under_a_moving_global() {
    let Some((device, queue)) = device("the masks' white under a preview check") else {
        return;
    };
    let (w, h) = (24usize, 8usize);
    let data: Vec<f32> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let v = 0.02 + 0.025 * x;
            [v * (1.2 + 0.1 * y), v * 0.9, v * (0.6 - 0.05 * y)]
        })
        .map(|v| half::f16::from_f32(v).to_f32())
        .collect();
    let image = WorkingImage {
        width: w,
        height: h,
        data,
    };
    let shift = crate::testing::white_shift();
    let preview = shift
        .to_temp_tint(greycard_core::TempTint {
            cct: 6500.0,
            duv: 0.0,
        })
        .unwrap();
    let mut look = Look {
        white_balance: greycard_edit::LocalWhite::Absolute {
            temperature: 3200.0,
            tint: 0.0,
        },
        ..Look::default()
    };
    look.mixer.hue[1] = 25.0;
    look.mixer.saturation[1] = 0.8;
    look.mixer.saturation[0] = -0.5;
    let mut edit = greycard_edit::Edit::default();
    edit.adjustments.push(everywhere(look));
    // The GPU: the develop's picture, the preview's matrix, the
    // mask's made against the develop's white.
    let locals: Vec<Local> = edit
        .adjustments
        .iter()
        .map(|a| Local::of(a, vec![None], Some(&shift)))
        .collect();
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    let view = View {
        center: (w as f32 / 2.0, h as f32 / 2.0),
        plane: (w as f32, h as f32),
        frame_size: (w as f32, h as f32),
        locals: locals.clone(),
        white: preview,
        ..View::with_look(&edit)
    };
    let target = renderer.render(w as u32, h as u32, &view).0;
    let shown = renderer.read_back(&target).expect("read back");
    let gpu: Vec<u8> = shown.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
    // The CPU: the develop at the preview's white, and the mask's
    // matrix against that.
    let developed = WorkingImage {
        width: w,
        height: h,
        data: image
            .data
            .chunks(3)
            .flat_map(|px| greycard_core::color::apply3(&preview, [px[0], px[1], px[2]]))
            .collect(),
    };
    let inv = greycard_core::color::invert3(preview).unwrap();
    let cpu_locals: Vec<Local> = locals
        .iter()
        .map(|l| Local {
            baked: Baked {
                white: l.baked.white.map(|m| greycard_core::color::mul3(m, inv)),
                ..l.baked.clone()
            },
            ..l.clone()
        })
        .collect();
    let iw = w as f32;
    let cpu = |locals: &[Local]| {
        crate::finish::finish_with(
            &developed,
            None,
            &Baked::global(&edit, Source::SCENE),
            locals,
            |x, y| ((x as f32 + 0.5) / iw, (y as f32 + 0.5) / iw),
            |_, _| (0.0, None),
            None,
            None,
            None,
            &crate::export::Space::Srgb.matrix(),
            |v| (v * 255.0).round() as u8,
        )
    };
    let want = cpu(&cpu_locals);
    let worst = want.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max();
    assert!(worst <= Some(2), "{worst:?}: {want:?} against {gpu:?}");
}

/// Contrast summed to nothing and under: the picture's 0.5 and one
/// or two masks over everything at 0.5 sum to 0 and -0.5, which
/// both sides hold at `MIN_CONTRAST` (a power of zero took black to
/// mid grey on the CPU, and is undefined in the shader). Black
/// stays black, and the two sides agree over a ramp.
#[test]
fn stacked_flattenings_agree_on_both_sides() {
    let Some((device, queue)) = device("the stacked flattenings' check") else {
        return;
    };
    let (w, h) = (16usize, 2usize);
    let image = WorkingImage {
        width: w,
        height: h,
        data: (0..w * h)
            .flat_map(|i| {
                let v = if i % w == 0 {
                    0.0
                } else {
                    0.002 * 2f32.powf((i % w) as f32 * 0.6)
                };
                [v, v * 0.8, v * 1.1]
            })
            .collect(),
    };
    for n in [1, 2] {
        let mut edit = greycard_edit::Edit::default();
        edit.light.tone.contrast = 0.5;
        for _ in 0..n {
            let mut look = Look::default();
            look.light.tone.contrast = 0.5;
            edit.adjustments.push(everywhere(look));
        }
        let (cpu, gpu) = both_sides(&device, &queue, &image, &edit);
        assert_eq!(&cpu[..3], &[0, 0, 0], "{n}: the CPU's black");
        assert_eq!(&gpu[..3], &[0, 0, 0], "{n}: the GPU's black");
        let worst = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max();
        assert!(worst <= Some(2), "{n}: {cpu:?} against {gpu:?}");
    }
}

/// The viewport's shader against the export's finish over a few
/// hundred random edits, over a small frame of saturated colors,
/// near black and clipped highlights. Each edit draws every look
/// slider the viewport previews over the range its panel gives it,
/// a fifth of the time at each end (the Light section and its
/// switch, the point, parametric and color curves, the grading,
/// the mixer, the color, the black and white, the tint, the
/// vignette and the grain), two or three local adjustments with
/// their own looks, half of them with a white balance of their own
/// (through `testing::white_shift`'s profile), under masks of
/// gradients, radials, brushes, a
/// learned raster (Subject) present or missing, and luminance and
/// color windows, in every mode, a look table, a source of a scene
/// or a picture already rendered, an output space and a guide
/// plane; and each mask's Detail, read from a random set of detail
/// planes.
///
/// Both sides are built from the edit the way the app builds them:
/// the locals by `Local::of`, as the viewport and the export bake
/// them, the view's look by `View::with_look`, the guide handed to
/// the CPU only where `export::reads_guide` says the export reads
/// it, the vignette and the grain by frame position as
/// `export::render` has them. The CPU is handed the picture, the
/// guide and the look tables the GPU holds, rounded to half
/// floats. Every encoded channel of every pixel is held to 2.5
/// levels of the CPU's bracket (`NUDGE`).
///
/// Not covered: the white balance the viewport previews
/// (`View::white`), the geometry (zoom, turns, crop, perspective,
/// the cubic reader), the export's mask sample from before its
/// output sharpen (`sampled`), the Background, Sky and Object
/// shapes (rasters as Subject's is, not drawn), the mask and
/// clipping overlays, the display table and the soft proof, and
/// the encoded picture's path.
///
/// `GREYCARD_PARITY_SEED` and `GREYCARD_PARITY_EDITS` change the
/// run; `GREYCARD_PARITY_ONLY` replays one edit, and
/// `GREYCARD_PARITY_SKIP` (names from `PARTS`, by commas) leaves
/// parts at their defaults. The first three failing edits are drawn
/// again with each part left at its default in turn, and the parts
/// whose removal brings them within the tolerance are named; an
/// amplifier's removal does that as well as a cause's.
#[test]
fn the_shader_is_the_cpu_over_random_edits() {
    let Some((device, queue)) = device("the random parity check") else {
        return;
    };
    let env = |name: &str| {
        std::env::var(name).ok().and_then(|v| {
            let v = v.trim();
            match v.strip_prefix("0x") {
                Some(hex) => u64::from_str_radix(hex, 16).ok(),
                None => v.parse().ok(),
            }
        })
    };
    let seed = env("GREYCARD_PARITY_SEED").unwrap_or(0x6772_6579);
    let edits = env("GREYCARD_PARITY_EDITS").unwrap_or(400);
    let only = env("GREYCARD_PARITY_ONLY");
    let skip = std::env::var("GREYCARD_PARITY_SKIP")
        .map(|names| {
            names
                .split(',')
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .fold(0, |s, n| {
                    assert!(
                        PARTS.contains(&n),
                        "GREYCARD_PARITY_SKIP: no part {n:?}; the parts are {PARTS:?}"
                    );
                    s | part(n)
                })
        })
        .unwrap_or(0);
    let started = std::time::Instant::now();
    let image = parity_frame();
    let (w, h) = (image.width, image.height);
    let aspect = h as f32 / w as f32;
    let seen = WorkingImage {
        width: w,
        height: h,
        data: image
            .data
            .iter()
            .map(|v| half::f16::from_f32(*v).to_f32())
            .collect(),
    };
    // Two guide planes: the picture's own, which at this size is a
    // texel a pixel, so `guide_at` never falls between texels; and
    // its mean over blocks of four, as a frame larger than
    // `GUIDE_EDGE` has it, read between texels everywhere.
    // Half floats on the GPU (`set_guide`), so the CPU is handed
    // them rounded the same way, as it is the picture and the look
    // tables.
    let native = crate::finish::guide_plane(&seen);
    let coarse = {
        let s = 4;
        let (gw, gh) = (w / s, h / s);
        let data: Vec<f32> = (0..gh)
            .flat_map(|y| (0..gw).map(move |x| (x, y)))
            .map(|(x, y)| {
                let block = (0..s * s).map(|k| native.data[(y * s + k / s) * w + x * s + k % s]);
                block.sum::<f32>() / (s * s) as f32
            })
            .collect();
        crate::finish::Guide {
            width: gw,
            height: gh,
            scale: s,
            range: crate::finish::plane_range(&data),
            data,
        }
    };
    let guides = [native, coarse].map(|mut g| {
        for v in &mut g.data {
            *v = half::f16::from_f32(*v).to_f32();
        }
        g
    });
    let pool = raster_pool(seed, aspect);
    let tables = look_tables();
    let shift = crate::testing::white_shift();
    let planes = Arc::new(parity_planes(w, h));
    let planes_image = planes.to_image();
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&crate::worker::Halves::from_image(&image, None));
    // The picture with each channel in turn moved by `NUDGE` of the
    // pixel's largest, up and down.
    // `FINEST` is the last step's answers: the steps run from the
    // largest down, both signs of each, three channels of each sign.
    assert!(NUDGES.windows(2).all(|p| p[0] > p[1]));
    let nudged: Vec<WorkingImage> = NUDGES
        .into_iter()
        .flat_map(|step| [step, -step])
        .flat_map(|step| (0..3).map(move |c| (step, c)))
        .map(|(step, c)| {
            let mut n = seen.clone();
            for px in n.data.chunks_mut(3) {
                let top = px.iter().fold(0.0f32, |a, &b| a.max(b));
                px[c] += step * NUDGE * top;
            }
            n
        })
        .collect();
    assert_eq!(nudged.len(), NUDGES.len() * FINEST);
    // The finer nudges, for a channel the GPU puts between two of
    // the answers and near none (`REFINED`).
    let fine: Vec<WorkingImage> = (0..REFINED)
        .map(|k| 0.03f32.powf(k as f32 / (REFINED - 1) as f32))
        .flat_map(|step| [step, -step])
        .flat_map(|step| (0..4).map(move |c| (step, c)))
        .map(|(step, c)| {
            let mut n = seen.clone();
            for px in n.data.chunks_mut(3) {
                let top = px.iter().fold(0.0f32, |a, &b| a.max(b));
                // Each channel alone, and all three together.
                for (k, v) in px.iter_mut().enumerate() {
                    if c == 3 || c == k {
                        *v += step * NUDGE * top;
                    }
                }
            }
            n
        })
        .collect();
    // The lit pixels: every channel holds more than the largest
    // nudge puts into it. In the others a channel is empty, or as
    // good as, and the nudge is not a small change of it but light
    // where there was none, which a curve steep at zero (a point at
    // the panel's 0.01, the encoding's 12.92 under it) makes whole
    // levels: an edit with such a curve makes every one of them
    // steep, the same on any device.
    let lit: Vec<bool> = seen
        .data
        .chunks(3)
        .map(|px| {
            let top = px.iter().fold(0.0f32, |a, &b| a.max(b));
            top > 0.0 && px.iter().all(|&v| v > NUDGE * top)
        })
        .collect();
    // One case through both sides.
    let mut run = |case: &Case| -> Outcome {
        let edit = &case.edit;
        let locals: Vec<Local> = edit
            .adjustments
            .iter()
            .zip(&case.rasters)
            .take(MAX_LOCALS)
            // A raw's locals have their white balance; a picture
            // already rendered has none to move (`local_white_shift`).
            .map(|(a, rasters)| {
                let white = (case.source.is_scene()).then_some(&shift);
                Local::of(a, rasters.clone(), white)
            })
            .collect();
        renderer.set_look(case.look.as_ref());
        renderer.set_output(case.space.matrix());
        let guide = case.guide.map(|k| &guides[k]);
        renderer.set_guide(guide.unwrap_or(&crate::finish::Guide::NONE));
        renderer.set_detail(case.planes.then_some(&planes));
        let detail = case.planes.then_some((&planes_image, &planes.meta));
        let view = View {
            center: (w as f32 / 2.0, h as f32 / 2.0),
            plane: (w as f32, h as f32),
            frame_size: (w as f32, h as f32),
            locals: locals.clone(),
            source: case.source,
            ..View::with_look(edit)
        };
        let target = renderer.render(w as u32, h as u32, &view).0;
        let shown = renderer.read_back(&target).expect("read back");
        // The export's side, as `export::render` sets it up: the
        // guide only where it says the export reads one.
        let reads = crate::export::reads_guide(edit);
        let global = Baked::global(edit, case.source);
        let (iw, ih) = (w as f32, h as f32);
        let vignette = edit.vignette;
        let grain = edit.grain;
        let frame_at = |x: usize, y: usize| {
            let (u, v) = ((x as f32 + 0.5) / iw, (y as f32 + 0.5) / ih);
            let stops = if vignette.is_off() {
                0.0
            } else {
                vignette.amount * vignette.at(u, v, iw / ih)
            };
            let noise = (!grain.is_off()).then(|| grain.at(u, v, iw / ih));
            (stops, noise)
        };
        let look = case.look.as_ref().filter(|l| !l.is_off());
        let finish = |image: &WorkingImage, global: &Baked| {
            crate::finish::finish_with(
                image,
                None,
                global,
                &locals,
                |x, y| ((x as f32 + 0.5) / iw, (y as f32 + 0.5) / iw),
                frame_at,
                guide.filter(|_| reads).map(|g| (g, iw)),
                detail,
                look,
                &case.space.matrix(),
                |v: f32| v,
            )
        };
        let cpu = finish(&seen, &global);
        let around: Vec<Vec<f32>> = nudged.iter().map(|n| finish(n, &global)).collect();
        let mut refined: Option<Vec<Vec<f32>>> = None;
        let mut out = Outcome {
            worst: 0.0,
            at: (0, 0, 0),
            cpu: [0.0; 3],
            gpu: [0.0; 3],
            steep: 0,
            steep_lit: 0,
            wide: 0,
            unsettled: 0,
            refined: 0,
            answers: Vec::new(),
        };
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                let gpu = shown.get_pixel(x as u32, y as u32);
                let gpu = [0, 1, 2].map(|c| f32::from(gpu[c]) / 255.0);
                let here = [cpu[i], cpu[i + 1], cpu[i + 2]];
                let mut steep = false;
                for c in 0..3 {
                    // The CPU's answers: the picture as it is and
                    // nudged. The GPU is held to the nearest of
                    // them, not to anything between the least and
                    // the most, so a bracket spread over the whole
                    // scale cannot pass a black. A NaN in the CPU's
                    // own answer fails.
                    let answers =
                        || std::iter::once(here[c]).chain(around.iter().map(|a| a[i + c]));
                    let (lo, hi) =
                        answers().fold((here[c], here[c]), |(lo, hi), v| (lo.min(v), hi.max(v)));
                    steep |= hi - lo >= STEEP;
                    out.wide += usize::from(hi - lo > WIDE);
                    // Where the finest nudges alone, a few roundings
                    // of the largest channel, spread the answers
                    // wider than the tolerance, the CPU's own answer
                    // is not settled at f32's precision, and the GPU
                    // is held to their range instead (`FINEST`).
                    let (flo, fhi) = std::iter::once(here[c])
                        .chain(around[around.len() - FINEST..].iter().map(|a| a[i + c]))
                        .fold((here[c], here[c]), |(lo, hi), v| (lo.min(v), hi.max(v)));
                    let unsettled = fhi - flo > TOLERANCE;
                    out.unsettled += usize::from(unsettled);
                    let mut d = if here[c].is_nan() {
                        f32::NAN
                    } else if unsettled {
                        (lo - gpu[c]).max(gpu[c] - hi).max(0.0)
                    } else {
                        answers()
                            .map(|v| (v - gpu[c]).abs())
                            .fold(f32::INFINITY, f32::min)
                    };
                    // Between two answers and near none: the finer
                    // sampling of the same neighborhood decides
                    // (`REFINED`).
                    if d > TOLERANCE && gpu[c] > lo && gpu[c] < hi {
                        let refined = refined.get_or_insert_with(|| {
                            fine.iter().map(|n| finish(n, &global)).collect()
                        });
                        d = refined
                            .iter()
                            .map(|a| (a[i + c] - gpu[c]).abs())
                            .fold(d, f32::min);
                        out.refined += 1;
                    }
                    if d > out.worst || (d.is_nan() && !out.worst.is_nan()) {
                        out.worst = d;
                        out.at = (x, y, c);
                        out.cpu = here;
                        out.gpu = gpu;
                        out.answers = answers().collect();
                    }
                }
                out.steep += usize::from(steep);
                out.steep_lit += usize::from(steep && lit[y * w + x]);
            }
        }
        out
    };
    let tolerance = TOLERANCE;
    let pixels = (w * h) as f64;
    let lit_pixels = lit.iter().filter(|l| **l).count() as f64;
    let mut failed = Vec::new();
    let mut overall = 0.0f32;
    let mut steep_sum = 0.0f64;
    let mut wide = 0usize;
    let mut steepest = (0.0f64, 0u64);
    let mut steepest_lit = (0.0f64, 0u64);
    let mut unsettled = 0usize;
    let mut most_unsettled = (0usize, 0u64);
    let mut refined = 0usize;
    let range = match only {
        Some(k) => k..k + 1,
        None => 0..edits,
    };
    for k in range {
        let case = random_case(seed, k, skip, aspect, &pool, &tables);
        let Outcome {
            worst: d,
            at: (x, y, c),
            cpu,
            gpu,
            steep,
            steep_lit,
            wide: n,
            unsettled: u,
            refined: r,
            answers,
        } = run(&case);
        wide += n;
        unsettled += u;
        if u > most_unsettled.0 {
            most_unsettled = (u, k);
        }
        refined += r;
        let share = steep as f64 / pixels;
        let share_lit = steep_lit as f64 / lit_pixels;
        steep_sum += share;
        if share > steepest.0 {
            steepest = (share, k);
        }
        if share_lit > steepest_lit.0 {
            steepest_lit = (share_lit, k);
        }
        if !d.is_nan() {
            overall = overall.max(d);
        }
        let too_steep = share_lit > STEEP_SHARE || u > UNSETTLED_MAX;
        if d <= tolerance && !too_steep {
            continue;
        }
        let i = (y * w + x) * 3;
        eprintln!(
            "seed {seed:#x} edit {k}: {:.1} levels from the CPU's answers at {x},{y} \
             channel {c} (cpu {cpu:.4?}, gpu {gpu:.4?}); the pixel {:?}; \
             {:.1}% of pixels steep, {:.1}% of the lit ones; {u} channels unsettled",
            d * 255.0,
            &seen.data[i..i + 3],
            100.0 * share,
            100.0 * share_lit
        );
        eprintln!(
            "  the CPU's answers there, in levels: {:?}",
            answers
                .iter()
                .map(|v| (v * 255.0 * 10.0).round() / 10.0)
                .collect::<Vec<_>>()
        );
        // Which part it is in, for the first few: drawn again
        // without each.
        let mut parts = Vec::new();
        if failed.len() < 3 {
            for (b, name) in PARTS.iter().enumerate() {
                let without = random_case(seed, k, skip | 1 << b, aspect, &pool, &tables);
                let o = run(&without);
                if o.worst <= tolerance
                    && o.steep_lit as f64 / lit_pixels <= STEEP_SHARE
                    && o.unsettled <= UNSETTLED_MAX
                {
                    parts.push(*name);
                }
            }
            eprintln!("  within tolerance without: {parts:?}");
        }
        if only.is_some() {
            eprintln!("  {:#?}", case.edit);
            eprintln!(
                "  look {:?}, source {:?}, guide {:?}, space {:?}",
                case.look.as_ref().map(|l| (l.lut.size, l.strength)),
                case.source,
                case.guide,
                case.space
            );
        }
        failed.push((k, d, share_lit, parts));
    }
    let count = if only.is_some() { 1 } else { edits };
    let mean = steep_sum / count as f64;
    eprintln!(
        "{count} edits, GPU against CPU: at most {:.2} levels from the CPU's answers, \
         {} failing; steep pixels {:.3}% on average, most {:.2}% (edit {}), \
         of the lit ones most {:.2}% (edit {}); {wide} channels' answers over {:.0} \
         levels apart, {unsettled} unsettled, most {} (edit {}), {refined} refined; \
         in {:.1} s",
        overall * 255.0,
        failed.len(),
        100.0 * mean,
        100.0 * steepest.0,
        steepest.1,
        100.0 * steepest_lit.0,
        steepest_lit.1,
        WIDE * 255.0,
        most_unsettled.0,
        most_unsettled.1,
        started.elapsed().as_secs_f64()
    );
    // A single edit replayed has no run to hold to the mean.
    assert!(
        only.is_some() || mean <= STEEP_MEAN,
        "seed {seed:#x}: {:.2}% of pixels steep over the run",
        100.0 * mean
    );
    let unsettled_mean = unsettled as f64 / count as f64;
    assert!(
        only.is_some() || unsettled_mean <= UNSETTLED_MEAN,
        "seed {seed:#x}: {unsettled_mean:.1} channels an edit unsettled over the run"
    );
    assert!(
        failed.is_empty(),
        "seed {seed:#x}: {} edits failing (over {:.1} levels, over {:.0}% of the lit pixels \
         steep, or over {UNSETTLED_MAX} channels unsettled): {:?}",
        failed.len(),
        tolerance * 255.0,
        STEEP_SHARE * 100.0,
        failed
            .iter()
            .map(|(k, d, s, p)| format!(
                "{k} ({:.1} levels, {:.1}% of the lit steep: {p:?})",
                d * 255.0,
                s * 100.0
            ))
            .collect::<Vec<_>>()
    );
}
