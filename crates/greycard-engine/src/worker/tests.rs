use super::*;
use crate::stages::Keep;

#[test]
fn the_strip_s_own_frames_are_made_first_then_the_ones_beside_them() {
    let mut pending: Vec<(usize, PathBuf)> = (0..10)
        .map(|i| (i, PathBuf::from(format!("{i}"))))
        .collect();
    order_thumbnails(&mut pending, 4, 6);
    let order: Vec<usize> = pending.iter().map(|(i, _)| *i).collect();
    // The range in file order, then outward from its middle, 5.
    assert_eq!(order, vec![4, 5, 6, 3, 7, 2, 8, 1, 9, 0]);
}

/// Run a set of `names` (files of junk, which nothing decodes)
/// through a worker of its own, cancelling the set as frame
/// `cancel_at` is begun, and asking for an open (of junk, generation
/// 7) as frame `open_at` is; what the worker said, in order, in
/// short.
fn run_fake_set(
    tag: &str,
    names: &[&str],
    cancel_at: Option<usize>,
    open_at: Option<usize>,
) -> Vec<String> {
    let dir = thumb_scratch(tag);
    let junk = dir.join("junk.CR3");
    std::fs::write(&junk, vec![0x33u8; 4096]).unwrap();
    // The worker's queue, for the deliver to put a job on as a
    // window would, mid-set.
    type Shared = Arc<(Mutex<Queue>, Condvar)>;
    let slot: Arc<std::sync::OnceLock<Shared>> = Arc::new(std::sync::OnceLock::new());
    let seen = slot.clone();
    let frames: Vec<crate::queue::Frame> = names
        .iter()
        .map(|n| {
            let source = dir.join(n);
            std::fs::write(&source, vec![0x5au8; 4096]).unwrap();
            crate::queue::Frame {
                source: source.clone(),
                edit: Edit::default(),
                version: None,
                turn: 0,
                time_shift: 0,
                seed_blend: false,
                moved_from: None,
                out: dir.join("out").join(format!("{n}.jpg")),
            }
        })
        .collect();
    let set = Arc::new(crate::queue::Set::new(
        frames.len(),
        Some(dir.join("out")),
        greycard_render::export::Settings::default(),
        greycard_render::export::OnExists::Increment,
    ));
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        let line = match o {
            Outcome::SetFrameStarted { set, index, name } => {
                // Pressed while this frame is in hand: the deliver
                // runs on the worker's thread, before the work.
                if Some(index) == cancel_at {
                    set.cancel();
                }
                if Some(index) == open_at {
                    let (lock, cv) = &**seen.get().expect("the queue is handed over");
                    lock.lock().unwrap().develop = Some(Job::Open {
                        path: junk.clone(),
                        version: None,
                        edit: Edit::default(),
                        generation: 7,
                        seed_blend: false,
                        turn: 0,
                        stance_known: false,
                        learned: Vec::new(),
                    });
                    cv.notify_one();
                }
                format!("begin {index} {name}")
            }
            Outcome::Failed { generation, .. } => format!("develop {generation} failed"),
            Outcome::SetFrameDone { index, done, .. } => match done {
                crate::queue::Done::Failed { message } => {
                    assert!(!message.is_empty());
                    format!("failed {index}")
                }
                crate::queue::Done::Canceled => format!("canceled {index}"),
                other => format!("{other:?} {index}"),
            },
            Outcome::SetDone { tally, .. } => format!(
                "done {} exported, {} failed, {} canceled",
                tally.exported,
                tally.failed.len(),
                tally.canceled
            ),
            _ => return,
        };
        tx.lock().unwrap().send(line).unwrap();
    });
    slot.set(worker.queue.clone()).ok().unwrap();
    worker.send(Job::ExportSet {
        set: set.clone(),
        frames,
    });
    let mut said = Vec::new();
    loop {
        let line = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the worker answers");
        let done = line.starts_with("done");
        said.push(line);
        if done {
            break;
        }
    }
    worker.stop();
    assert!(!dir.join("out").exists(), "nothing was written");
    crate::testing::remove_dir_retry(&dir);
    said
}

/// A set whose every frame fails: each is begun in order and is
/// one failure, the rest of the set goes on regardless, and the
/// set is said done once, after its last, with the count.
#[test]
fn a_set_runs_in_order_and_counts_its_failures() {
    let said = run_fake_set("setfail", &["A.CR3", "B.CR3", "C.CR3"], None, None);
    assert_eq!(
        said,
        vec![
            "begin 0 A.CR3.jpg",
            "failed 0",
            "begin 1 B.CR3.jpg",
            "failed 1",
            "begin 2 C.CR3.jpg",
            "failed 2",
            "done 0 exported, 3 failed, 0 canceled",
        ]
    );
}

/// Canceled while the second of four is in hand: that one is
/// finished, the last two are passed over without being begun.
#[test]
fn a_cancelled_set_finishes_the_frame_in_hand_and_stops() {
    let said = run_fake_set(
        "setcancel",
        &["A.CR3", "B.CR3", "C.CR3", "D.CR3"],
        Some(1),
        None,
    );
    assert_eq!(
        said,
        vec![
            "begin 0 A.CR3.jpg",
            "failed 0",
            "begin 1 B.CR3.jpg",
            "failed 1",
            "canceled 2",
            "canceled 3",
            "done 0 exported, 2 failed, 2 canceled",
        ]
    );
}

/// A stop that finds the worker still in a job says so and keeps
/// its thread, so the quit can tell it is there and not run the C
/// library's exit under it; once out of the job the worker sees
/// the stop, ends, and is joined. The job held here is an open of
/// junk whose failure the deliver sits on, on the worker's thread,
/// as a develop sits in the driver.
#[test]
fn a_stop_that_finds_the_worker_in_a_job_keeps_it_until_it_ends() {
    let dir = thumb_scratch("busystop");
    let junk = dir.join("junk.CR3");
    std::fs::write(&junk, vec![0x33u8; 4096]).unwrap();
    let (entered, in_job) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (entered, released) = (Mutex::new(entered), Mutex::new(released));
    let worker = Worker::new(move |o| {
        if let Outcome::Failed { .. } = o {
            entered.lock().unwrap().send(()).unwrap();
            let _ = released.lock().unwrap().recv();
        }
    });
    assert!(!worker.finished(), "a worker not asked to stop is there");
    worker.send(Job::Open {
        path: junk,
        version: None,
        edit: Edit::default(),
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    in_job
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the worker takes the open");
    assert!(
        !worker.stop_within(std::time::Duration::from_millis(50)),
        "the stop finds it in its job"
    );
    assert!(worker.running(), "its thread is kept");
    assert!(!worker.finished());
    release.send(()).unwrap();
    let asked = Instant::now();
    while !worker.finished() {
        assert!(
            asked.elapsed() < std::time::Duration::from_secs(30),
            "out of its job, the worker ends"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!worker.running());
    assert!(worker.stop(), "a stop after the end has nothing to wait on");
    crate::testing::remove_dir_retry(&dir);
}

/// A stop that finds the worker between jobs joins it at once.
#[test]
fn a_stop_between_jobs_joins_the_worker() {
    let worker = Worker::new(|_| {});
    assert!(worker.stop());
    assert!(worker.finished());
    assert!(!worker.running());
}

/// A develop asked for while a frame of a set is in hand goes
/// before the set's next frame: the set waits behind the window.
#[test]
fn a_develop_asked_for_mid_set_goes_before_the_next_frame() {
    let said = run_fake_set("setdevelop", &["A.CR3", "B.CR3", "C.CR3"], None, Some(0));
    assert_eq!(
        said,
        vec![
            "begin 0 A.CR3.jpg",
            "failed 0",
            "develop 7 failed",
            "begin 1 B.CR3.jpg",
            "failed 1",
            "begin 2 C.CR3.jpg",
            "failed 2",
            "done 0 exported, 3 failed, 0 canceled",
        ]
    );
}

#[test]
fn a_panic_in_a_job_is_the_failure_that_job_was_waited_on_for() {
    let develop = Job::Develop {
        edit: Edit::default(),
        generation: 7,
        turn: 0,
        of: None,
    };
    // The panics here are on purpose; the hook has nothing to say
    // about them. The hook is the process's, and the other tests
    // run beside this one: theirs still print, or a failure
    // elsewhere in the run would be a bare FAILED with no words.
    let hook = std::panic::take_hook();
    let mine = std::thread::current().id();
    let hushed: std::sync::Arc<std::sync::Mutex<Option<_>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Some(hook)));
    let shared = hushed.clone();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() != mine
            && let Ok(guard) = shared.lock()
            && let Some(hook) = guard.as_ref()
        {
            hook(info);
        }
    }));
    let payload = std::panic::catch_unwind(|| panic!("on purpose {}", 1)).unwrap_err();
    let plain = std::panic::catch_unwind(|| panic!("plain")).unwrap_err();
    let _ = std::panic::take_hook();
    if let Some(hook) = hushed.lock().unwrap().take() {
        std::panic::set_hook(hook);
    }
    let message = panic_message(payload.as_ref());
    assert_eq!(message, "on purpose 1");
    match Blame::of(&develop).outcome(message) {
        Some(Outcome::Failed {
            generation,
            message,
        }) => {
            assert_eq!(generation, 7);
            assert!(message.contains("panicked: on purpose 1"), "{message}");
        }
        _ => panic!("not the develop's failure"),
    }
    let mask = Job::Mask {
        key: (3, 1),
        shape: Shape::Linear {
            from: [0.0, 0.0],
            to: [1.0, 1.0],
        },
        model: None,
    };
    assert!(matches!(
        Blame::of(&mask).outcome("x".into()),
        Some(Outcome::MaskFailed { key: (3, 1), .. })
    ));
    let thumbnail = Job::Thumbnail {
        index: 0,
        frame: Frame::file("a"),
    };
    assert_eq!(Blame::of(&thumbnail), Blame::Nobody);
    assert!(Blame::Nobody.outcome("x".into()).is_none());
    assert_eq!(panic_message(plain.as_ref()), "plain");
}

/// A Bayer frame of a textured scene with lateral CA in it, large
/// enough for the correction to run, in sensor units.
pub(super) fn aberrated_frame(width: usize, height: usize) -> RawFrame {
    use greycard_core::raw::{
        Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Samples, SensorLayout,
    };
    let pattern = CfaPattern::rggb();
    let texture = |x: f32, y: f32| {
        let v = (x * 0.11).sin() * (y * 0.07).cos()
            + (x * 0.031 + y * 0.052).sin()
            + ((x * 0.9 + y * 0.37) * 0.23).sin() * 0.5
            + ((x - y) * 0.017).cos() * 0.7;
        0.5 + 0.3 * v / 3.2
    };
    let (cx, cy) = ((width - 1) as f32 / 2.0, (height - 1) as f32 / 2.0);
    let mut samples = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let (scale, gain) = match pattern.color_at(y, x) {
                CfaColor::Red => (1.005, 0.5),
                CfaColor::Blue => (0.996, 0.25),
                _ => (1.0, 1.0),
            };
            let sx = cx + (x as f32 - cx) / scale;
            let sy = cy + (y as f32 - cy) / scale;
            samples.push(100 + (texture(sx, sy) * gain * 1000.0) as u16);
        }
    }
    let m = greycard_core::color::WORKING_SPACE
        .from_xyz_matrix()
        .expect("the working space has a matrix");
    let color_matrix = m.rows.iter().flatten().map(|v| *v as f32).collect();
    RawFrame {
        make: "Test".into(),
        model: "Cam".into(),
        other_names: Vec::new(),
        width,
        height,
        channels: 1,
        layout: SensorLayout::Cfa(pattern),
        samples: Samples::U16(samples),
        levels: Levels {
            black: LevelPattern::uniform(100.0, 1),
            white: LevelPattern::uniform(1100.0, 1),
            black_deltas: None,
            saturation: None,
        },
        as_shot_coefficients: Some([2.0, 1.0, 4.0]),
        calibrations: vec![Calibration {
            illuminant: 21,
            color_matrix,
            forward_matrix: None,
        }],
        crop: None,
        orientation: greycard_core::raw::Orientation::Normal,
        shot: Default::default(),
        dng: Default::default(),
    }
}

/// `develop_job` on `frame` under the default edit with `gpu`,
/// from `base`: the outcome and the picture.
fn develop_once(
    frame: &Arc<RawFrame>,
    base: &mut Option<FrameKept>,
    gpu: &mut Option<greycard_gpu::Context>,
) -> (Outcome, Option<Arc<WorkingImage>>) {
    let deliver: Deliver = Arc::new(|_| {});
    develop_job(
        &Input::Raw(frame.clone()),
        &Edit::default(),
        0,
        1,
        base,
        &mut Ai::new(),
        None,
        None,
        gpu,
        &deliver,
    )
}

/// `develop_job` on `frame` under `edit` with `gpu`, from `base`: the
/// outcome and the picture.
fn develop_with(
    frame: &Arc<RawFrame>,
    edit: &Edit,
    base: &mut Option<FrameKept>,
    gpu: &mut Option<greycard_gpu::Context>,
) -> (Outcome, Option<Arc<WorkingImage>>) {
    let deliver: Deliver = Arc::new(|_| {});
    develop_job(
        &Input::Raw(frame.clone()),
        edit,
        0,
        1,
        base,
        &mut Ai::new(),
        None,
        None,
        gpu,
        &deliver,
    )
}

/// A device error in the demosaic (a card too small for its working
/// set says so as an error of its own) falls back to the CPU's
/// demosaic for that frame and keeps the context: a device allowed 20
/// workgroups a dimension cannot run a 330-wide mosaic's 21, and the
/// frame is too small for the CA to run at all.
#[test]
fn a_gpu_error_in_the_demosaic_falls_back_and_keeps_the_context() {
    use crate::stages::{Keep, Purpose, Stage, Step};
    use greycard_gpu::wgpu::Limits;
    let Some(ctx) = context_with(Limits {
        max_compute_workgroups_per_dimension: 20,
        ..Limits::default()
    }) else {
        skipped("the demosaic fallback check");
        return;
    };
    let input = crate::walk::Input::Raw(Arc::new(aberrated_frame(330, 200)));
    let edit = Edit::default();
    let mut steps = crate::stages::stages(&edit, &Purpose::Viewport, &input.facts(0), None);
    let base_at = steps.at(Keep::Base).unwrap();
    steps.steps.truncate(base_at + 1);
    let demosaic = steps
        .steps
        .iter()
        .position(|s| matches!(s, Step::Op(Stage::Demosaic(..))))
        .unwrap();
    let places = crate::walk::plan(&steps, &Purpose::Viewport, true);
    assert_eq!(places[demosaic], crate::walk::Place::Gpu);
    let mut gpu = Some(ctx);
    let mut kept = crate::walk::Kept::new();
    let walked = crate::walk::walk(
        &steps,
        &places,
        &input,
        &mut kept,
        &mut crate::walk::NoModels,
        &mut gpu,
        &crate::walk::NoHold,
    )
    .expect("the develop goes on");
    assert_eq!(walked.places[demosaic], crate::walk::Place::Cpu);
    assert!(gpu.is_some(), "the context is kept");
    let mut reference = crate::walk::Kept::new();
    crate::walk::walk(
        &steps,
        &crate::walk::plan(&steps, &Purpose::Viewport, false),
        &input,
        &mut reference,
        &mut crate::walk::NoModels,
        &mut None,
        &crate::walk::NoHold,
    )
    .unwrap();
    assert!(kept.picture(Keep::Base).unwrap().data == reference.picture(Keep::Base).unwrap().data);
}

/// `develop_job` on `frame` under `edit` at the frame's `turn`,
/// on the CPU, from `base`: the picture.
fn develop_turned(
    frame: &Arc<RawFrame>,
    edit: &Edit,
    turn: u8,
    base: &mut Option<FrameKept>,
) -> Arc<WorkingImage> {
    let deliver: Deliver = Arc::new(|_| {});
    let (outcome, image) = develop_job(
        &Input::Raw(frame.clone()),
        edit,
        turn,
        1,
        base,
        &mut Ai::new(),
        None,
        None,
        &mut None,
        &deliver,
    );
    assert!(matches!(outcome, Outcome::Developed { .. }));
    image.expect("a picture on the CPU")
}

/// The open file's export of `frame` under `edit`, walked from what
/// `base` keeps of the window's develops: its pixels, and what of the
/// edit it went without.
fn export_held(
    frame: &Arc<RawFrame>,
    edit: &Edit,
    base: Option<&FrameKept>,
    ai: &mut Ai,
    settings: &greycard_render::export::Settings,
) -> (greycard_render::export::Rendered, Vec<String>) {
    let deliver: Deliver = Arc::new(|_| {});
    super::export::render_open(
        edit,
        0,
        &Input::Raw(frame.clone()),
        base,
        &mut WorkerModels::new(ai, None, &deliver, 0),
        None,
        &mut None,
        settings,
    )
    .expect("exported")
}

/// A develop that finds its base at another turn turns it rather than
/// making it again, and the base, its guide plane, and the develop are
/// the ones a develop at the new turn makes, to the bit: for a frame
/// whose tag is a plain turn and one whose tag mirrors, for a quarter
/// each way and a half.
#[test]
fn a_base_turned_in_place_is_the_base_developed_at_that_turn() {
    use greycard_core::raw::Orientation;
    let edit = Edit::default();
    for tag in [Orientation::Normal, Orientation::FlipHorizontal] {
        let mut frame = aberrated_frame(120, 80);
        frame.orientation = tag;
        let frame = Arc::new(frame);
        for (from, to) in [(0u8, 1u8), (1, 0), (0, 3), (1, 3), (2, 0)] {
            let what = format!("{tag:?}, {from} to {to}");
            let mut base = None;
            develop_turned(&frame, &edit, from, &mut base);
            let made = base.as_ref().map(|b| b.stamp);
            let image = develop_turned(&frame, &edit, to, &mut base);
            let b = base.as_ref().expect("a base");
            assert_eq!(b.turn, to);
            assert_eq!(Some(b.stamp), made.map(|s| s + 1), "{what}");
            let mut fresh = None;
            let reference = develop_turned(&frame, &edit, to, &mut fresh);
            let fresh = fresh.expect("a base");
            assert_eq!(
                (b.image.width, b.image.height),
                (fresh.image.width, fresh.image.height),
                "{what}"
            );
            assert!(
                b.image.data == fresh.image.data,
                "{what}: the pixels differ"
            );
            assert_eq!(b.guide.data, fresh.guide.data, "{what}: the guide differs");
            assert!(image.data == reference.data, "{what}: the develop differs");
        }
    }
}

/// A context on a device of our own with `limits`, for driving
/// the GPU paths into their errors; none without an adapter.
fn context_with(limits: greycard_gpu::wgpu::Limits) -> Option<greycard_gpu::Context> {
    use greycard_gpu::wgpu;
    let instance = greycard_gpu::instance();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("worker test"),
        required_limits: limits,
        ..Default::default()
    }))
    .ok()?;
    greycard_gpu::Context::from_device(&device, &queue).ok()
}

/// A GPU test with no adapter says so and returns; under
/// `GREYCARD_REQUIRE_GPU` (CI's software adapter, lavapipe here)
/// that is a failure, as greycard-gpu's own tests have it.
pub(super) fn skipped(what: &str) {
    assert!(
        std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
        "{what} has no GPU to run on and GREYCARD_REQUIRE_GPU is set"
    );
    eprintln!("SKIPPED: {what} has no GPU to run on");
    println!("SKIPPED: {what} has no GPU to run on");
}

/// A develop with no device (an export under `--cpu`, or after the
/// device went) is the reference's: a base whose CA ran on the GPU is
/// not reused by it, the base is made afresh on the CPU, and that
/// picture is the one a session that never had a GPU makes, byte for
/// byte.
#[test]
fn an_export_after_a_gpu_base_develops_the_reference_s() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the export's base check");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let mut gpu = Some(ctx);
    let mut base = None;
    let (outcome, _) = develop_once(&frame, &mut base, &mut gpu);
    assert!(matches!(outcome, Outcome::Developed { .. }));
    assert!(base.as_ref().is_some_and(|b| b.on_gpu));
    let stamp = base.as_ref().unwrap().stamp;
    // No GPU, the same edit.
    let (outcome, exported) = develop_once(&frame, &mut base, &mut None);
    assert!(matches!(outcome, Outcome::Developed { .. }));
    let b = base.as_ref().unwrap();
    assert!(!b.on_gpu && b.stamp == stamp + 1, "a fresh base on the CPU");
    // And once made on the CPU the base is kept.
    let (_, again) = develop_once(&frame, &mut base, &mut None);
    assert_eq!(base.as_ref().unwrap().stamp, stamp + 1);
    // The reference's own develop, from nothing.
    let (_, reference) = develop_once(&frame, &mut None, &mut None);
    let (exported, again, reference) = (exported.unwrap(), again.unwrap(), reference.unwrap());
    assert_eq!(exported.data, reference.data);
    assert_eq!(again.data, reference.data);
}

/// The upload is of the very picture a run on the device starts from:
/// after a develop without the device (the open file's export) made
/// the base again with the CA on the CPU, the upload of the old one is
/// let go with it, and the window's next develop uploads the new
/// picture rather than run on the GPU's, whose steps are the same; one
/// that changes nothing before the upload keeps it.
#[test]
fn a_picture_made_again_is_uploaded_again() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the upload's picture");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let mut gpu = Some(ctx);
    let mut base = None;
    let upload = |base: &Option<FrameKept>| {
        base.as_ref()
            .and_then(|b| b.kept.upload())
            .expect("uploaded")
            .clone()
    };
    develop_once(&frame, &mut base, &mut gpu);
    let first = upload(&base);
    develop_once(&frame, &mut base, &mut None);
    assert!(
        base.as_ref().unwrap().kept.upload().is_none(),
        "let go with its picture"
    );
    develop_once(&frame, &mut base, &mut gpu);
    let again = upload(&base);
    assert!(!Arc::ptr_eq(&again, &first), "the CPU's picture uploaded");
    develop_once(&frame, &mut base, &mut gpu);
    assert!(Arc::ptr_eq(&upload(&base), &again), "kept");
}

/// A learned pair whose CA ran on the GPU serves a develop with a
/// GPU and not one without, which makes the pair again on the CPU;
/// a pair whose CA was the CPU's serves both. The turn and the
/// learned settings still have to match.
#[test]
fn a_gpu_ca_learned_pair_is_not_served_to_an_export() {
    use crate::stages::{Keep, Stage};
    let mut edit = Edit::default();
    edit.noise.learned = greycard_edit::Learned::Fast;
    let input = Input::Raw(Arc::new(aberrated_frame(64, 48)));
    let walked = || FrameKept::walked_with(&input.walked(), &edit, 0, &mut LearnedFake);
    let serves = |b: &FrameKept, edit: &Edit, turn: u8, lent: bool| {
        let (steps, plan) = window_steps(edit, &b.facts_at(turn), None, lent);
        b.kept.serves(&steps, &plan, Keep::Pair)
    };
    let mut gpu = walked();
    gpu.kept.as_if_on_device(|s| matches!(s, Stage::Ca(_)));
    assert!(serves(&gpu, &edit, 0, true));
    assert!(!serves(&gpu, &edit, 0, false), "the export makes it again");
    let cpu = walked();
    assert!(serves(&cpu, &edit, 0, true));
    assert!(serves(&cpu, &edit, 0, false));
    assert!(!serves(&cpu, &edit, 1, false), "another turn");
    let mut other = edit.clone();
    other.noise.learned = greycard_edit::Learned::Best;
    assert!(!serves(&cpu, &other, 0, false), "other learned settings");
}

/// Models whose network is the plain demosaic, for a learned pair
/// without a model.
struct LearnedFake;

impl crate::walk::Models for LearnedFake {
    fn can_denoise(
        &mut self,
        _: &'static greycard_ai::Model,
    ) -> Result<(), crate::walk::NoLearned> {
        Ok(())
    }

    fn denoise(
        &mut self,
        _: &'static greycard_ai::Model,
        _: &RawFrame,
        prepared: &greycard_core::develop::Prepared,
    ) -> Result<Vec<f32>, crate::walk::NoLearned> {
        greycard_core::develop::demosaic_prepared(
            prepared,
            &greycard_core::develop::DevelopSettings::default(),
        )
        .map(|(rgb, _, _)| rgb)
        .map_err(|e| crate::walk::NoLearned::Failed(e.to_string()))
    }

    fn fill(
        &mut self,
        _: &greycard_edit::retouch::Patch,
        _: &greycard_core::develop::retouch::Region,
        _: &WorkingImage,
    ) -> Option<Vec<f32>> {
        None
    }

    fn could_fill_more(&mut self, _: &greycard_edit::retouch::Retouch, _: &WorkingImage) -> bool {
        false
    }

    fn rasters(
        &mut self,
        _: crate::stages::Masks,
        _: &Arc<WorkingImage>,
        _: Option<crate::walk::Patched<'_>>,
        _: &crate::walk::Meta,
    ) -> crate::walk::Rasters {
        crate::walk::Rasters::new()
    }
}

/// The export, with no window and no panel, makes a mask's white
/// balance from its own develop: the base keeps the profile its
/// white was resolved through, and a mask over everything at that
/// same white moves nothing while one at another moves the picture
/// the way the matrix says.
#[test]
fn an_export_makes_a_masks_white_balance_from_its_own_develop() {
    let frame = Arc::new(aberrated_frame(200, 160));
    // Developed at 5000 K, inside the range a mask's white takes.
    let global = Edit {
        white_balance: greycard_edit::WhiteBalance::Custom {
            temperature: 5000.0,
            tint: 0.002,
        },
        ..Edit::default()
    };
    let mut base = None;
    develop_turned(&frame, &global, 0, &mut base);
    assert!(
        base.as_ref().unwrap().white_shift().is_some(),
        "a raw's base keeps its profile"
    );
    let everywhere = |white: greycard_edit::LocalWhite| greycard_edit::Adjustment {
        id: 1,
        mask: greycard_edit::mask::Mask {
            components: vec![greycard_edit::mask::Component {
                shape: Shape::Linear {
                    from: [0.5, 10.0],
                    to: [0.5, 11.0],
                },
                ..Default::default()
            }],
            invert: false,
            ..Default::default()
        },
        look: greycard_edit::Look {
            white_balance: white,
            ..Default::default()
        },
        ..Default::default()
    };
    let settings = greycard_render::export::Settings {
        format: greycard_render::export::Format::Tiff,
        sharpen: greycard_render::export::Sharpen::Off,
        ..Default::default()
    };
    let mut ai = Ai::new();
    let mut render = |white| {
        let mut edit = global.clone();
        edit.adjustments.push(everywhere(white));
        let (r, _) = export_held(&frame, &edit, base.as_ref(), &mut ai, &settings);
        let greycard_render::export::Pixels::Sixteen(p) = r.pixels else {
            panic!("a TIFF is sixteen bits")
        };
        p
    };
    let off = render(greycard_edit::LocalWhite::Off);
    let same = render(greycard_edit::LocalWhite::Absolute {
        temperature: 5000.0,
        tint: 0.002,
    });
    let worst = off.iter().zip(&same).map(|(a, b)| a.abs_diff(*b)).max();
    assert!(worst <= Some(2), "{worst:?}");
    let warm = render(greycard_edit::LocalWhite::Absolute {
        temperature: 2800.0,
        tint: 0.0,
    });
    // Warmer light named: the picture comes out bluer.
    let blue = |p: &[u16]| {
        p.chunks(3).map(|c| c[2] as f64 - c[0] as f64).sum::<f64>() / (p.len() / 3) as f64
    };
    assert!(
        blue(&warm) > blue(&off) + 500.0,
        "{} {}",
        blue(&warm),
        blue(&off)
    );
    // Switched off with that light kept, it is the global's picture
    // to the last bit.
    let kept = render(greycard_edit::LocalWhite::Kept {
        temperature: 2800.0,
        tint: 0.0,
    });
    assert!(kept == off, "a kept light reaches no pixel");
}

/// A mask whose white follows the global keeps its offset when the
/// global moves, and a mask that names a light keeps the light: at a
/// 6500 K global, 46 mired warmer light is the picture a 5000 K lamp
/// makes; at a 5000 K global the same offset is another light, about
/// 4060 K, and the lamp is the global's own. No offset is the global's
/// picture at either.
#[test]
fn a_masks_offset_follows_the_global_and_its_light_does_not() {
    use greycard_edit::LocalWhite;
    let frame = Arc::new(aberrated_frame(200, 160));
    let everywhere = |white: LocalWhite| greycard_edit::Adjustment {
        id: 1,
        mask: greycard_edit::mask::Mask {
            components: vec![greycard_edit::mask::Component {
                shape: Shape::Linear {
                    from: [0.5, 10.0],
                    to: [0.5, 11.0],
                },
                ..Default::default()
            }],
            invert: false,
            ..Default::default()
        },
        look: greycard_edit::Look {
            white_balance: white,
            ..Default::default()
        },
        ..Default::default()
    };
    let settings = greycard_render::export::Settings {
        format: greycard_render::export::Format::Tiff,
        sharpen: greycard_render::export::Sharpen::Off,
        ..Default::default()
    };
    let mut ai = Ai::new();
    let mut render = |temperature: f64, white: LocalWhite| {
        let global = Edit {
            white_balance: greycard_edit::WhiteBalance::Custom {
                temperature,
                tint: 0.002,
            },
            ..Edit::default()
        };
        let mut base = None;
        develop_turned(&frame, &global, 0, &mut base);
        let mut edit = global.clone();
        edit.adjustments.push(everywhere(white));
        let (r, _) = export_held(&frame, &edit, base.as_ref(), &mut ai, &settings);
        let greycard_render::export::Pixels::Sixteen(p) = r.pixels else {
            panic!("a TIFF is sixteen bits")
        };
        p
    };
    let worst = |a: &[u16], b: &[u16]| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max();
    let blue = |p: &[u16]| {
        p.chunks(3).map(|c| c[2] as f64 - c[0] as f64).sum::<f64>() / (p.len() / 3) as f64
    };
    let lamp = LocalWhite::Absolute {
        temperature: 5000.0,
        tint: 0.002,
    };
    // Toward the lower kelvin, so negative.
    let cooler = LocalWhite::Relative {
        mired: 1e6 / 6500.0 - 1e6 / 5000.0,
        tint: 0.0,
    };
    let none = LocalWhite::Relative {
        mired: 0.0,
        tint: 0.0,
    };
    for global in [5000.0, 6500.0] {
        let off = render(global, LocalWhite::Off);
        let d = worst(&off, &render(global, none));
        assert!(d <= Some(2), "{global} K: {d:?}");
    }
    // At 6500 K the offset names the lamp's light.
    let d = worst(&render(6500.0, cooler), &render(6500.0, lamp));
    assert!(d <= Some(2), "{d:?}");
    // The global moved to 5000 K: the lamp is the global's own, the
    // offset moved with it to about 4060 K, a warmer light, so a bluer
    // picture under it.
    let off = render(5000.0, LocalWhite::Off);
    let d = worst(&off, &render(5000.0, lamp));
    assert!(d <= Some(2), "{d:?}");
    let followed = render(5000.0, cooler);
    assert!(
        blue(&followed) > blue(&off) + 500.0,
        "{} {}",
        blue(&followed),
        blue(&off)
    );
}

/// The open file's export walks from the window's points only where
/// the CPU made them: on the CPU it goes on from the window's developed
/// picture, and a base whose CA ran on the GPU is made again for it.
/// Either way the file is the one a set frame's walk from nothing
/// writes, and the window's points are left as they were.
#[test]
fn an_export_walks_from_the_window_s_cpu_points_and_leaves_them() {
    use crate::stages::Stage;
    let frame = Arc::new(aberrated_frame(400, 320));
    let settings = greycard_render::export::Settings {
        format: greycard_render::export::Format::Tiff,
        ..Default::default()
    };
    let mut ai = Ai::new();
    let deliver: Deliver = Arc::new(|_| {});
    let (fresh, _) = super::export::walk_export(
        Input::Raw(frame.clone()),
        &Edit::default(),
        0,
        &mut WorkerModels::new(&mut ai, None, &deliver, 0),
        None,
        &mut None,
        &settings,
    )
    .unwrap();
    let mut base = None;
    develop_once(&frame, &mut base, &mut None);
    let developed = base
        .as_ref()
        .and_then(|b| b.kept.picture(Keep::Developed))
        .expect("the CPU's walk keeps its developed picture")
        .clone();
    let (cpu, _) = export_held(&frame, &Edit::default(), base.as_ref(), &mut ai, &settings);
    same_pixels(&cpu, &fresh);
    let b = base.as_mut().unwrap();
    assert!(Arc::ptr_eq(
        b.kept.picture(Keep::Developed).unwrap(),
        &developed
    ));
    b.kept.as_if_on_device(|s| matches!(s, Stage::Ca(_)));
    let (gpu, _) = export_held(&frame, &Edit::default(), base.as_ref(), &mut ai, &settings);
    same_pixels(&gpu, &fresh);
    let b = base.as_ref().unwrap();
    assert!(
        b.kept
            .ran_on_device(Keep::Base, |s| matches!(s, Stage::Ca(_))),
        "the window's base as it was"
    );
    assert!(Arc::ptr_eq(
        b.kept.picture(Keep::Developed).unwrap(),
        &developed
    ));
}

/// An export of the open file leaves what the window's develop keeps on
/// the device where it is: the upload and the picture before the
/// sharpen, from which the window's next sharpen move goes on.
#[test]
fn an_export_leaves_the_window_s_device_points() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the window's device points across an export");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let deliver: Deliver = Arc::new(|_| {});
    let mut gpu = Some(ctx);
    let mut base = None;
    let mut edit = Edit::default();
    edit.detail.clarity = 0.5;
    let mut develop = |edit: &Edit, base: &mut Option<FrameKept>| {
        let (outcome, _) = develop_job(
            &Input::Raw(frame.clone()),
            edit,
            0,
            1,
            base,
            &mut Ai::new(),
            None,
            None,
            &mut gpu,
            &deliver,
        );
        let Outcome::Developed {
            detail: Some((_, ran)),
            ..
        } = outcome
        else {
            panic!("developed, with the local contrast");
        };
        ran
    };
    develop(&edit, &mut base);
    let b = base.as_ref().unwrap();
    assert!(b.kept.on_device(Keep::PreSharpen) && b.kept.has_upload());
    let uploaded = b.kept.upload().unwrap().clone();
    let settings = greycard_render::export::Settings::default();
    export_held(&frame, &edit, base.as_ref(), &mut Ai::new(), &settings);
    let b = base.as_ref().unwrap();
    assert!(
        b.kept.on_device(Keep::PreSharpen),
        "the picture before the sharpen"
    );
    assert!(
        Arc::ptr_eq(b.kept.upload().unwrap(), &uploaded),
        "the upload"
    );
    edit.sharpen.iterations += 1;
    assert_eq!(develop(&edit, &mut base), DetailRan::Kept);
}

/// A device error in the CA correction falls back to the reference
/// for that develop and drops the context for the session; a
/// picture the device cannot hold falls back for that op and keeps
/// it. A device allowed 20 workgroups a dimension cannot run the
/// 400x320's 26; one whose textures stop at 512 a side holds a
/// 500-wide picture but not the CA's padded green plane.
#[test]
fn a_gpu_error_in_the_ca_falls_back_and_an_unsupported_size_keeps_the_context() {
    use greycard_gpu::wgpu::Limits;
    let Some(ctx) = context_with(Limits {
        max_compute_workgroups_per_dimension: 20,
        ..Limits::default()
    }) else {
        skipped("the CA fallback check");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let mut gpu = Some(ctx);
    let mut base = None;
    // Bilinear, whose demosaic is the CPU's, so the CA is the one op
    // on the device.
    let bilinear = Edit {
        demosaic: greycard_edit::Demosaic::Bilinear,
        ..Edit::default()
    };
    let (outcome, image) = develop_with(&frame, &bilinear, &mut base, &mut gpu);
    assert!(matches!(outcome, Outcome::Developed { .. }));
    assert!(gpu.is_none(), "the context is dropped after a device error");
    assert!(base.as_ref().is_some_and(|b| !b.on_gpu));
    let (_, reference) = develop_with(&frame, &bilinear, &mut None, &mut None);
    assert_eq!(image.unwrap().data, reference.unwrap().data);

    let Some(ctx) = context_with(Limits {
        max_texture_dimension_2d: 512,
        ..Limits::default()
    }) else {
        skipped("the CA unsupported-size check");
        return;
    };
    let frame = Arc::new(aberrated_frame(500, 300));
    let mut gpu = Some(ctx);
    let mut base = None;
    let (outcome, _) = develop_with(&frame, &bilinear, &mut base, &mut gpu);
    assert!(matches!(outcome, Outcome::Developed { .. }));
    assert!(
        gpu.is_some(),
        "the context is kept for a picture it cannot hold"
    );
    assert!(base.as_ref().is_some_and(|b| !b.on_gpu));
}

/// An edit whose lens section moves the picture without a database:
/// the manual distortion and the manual aberration.
fn lens_edit() -> Edit {
    let mut edit = Edit::default();
    edit.lens.enabled = true;
    edit.lens.manual = -0.05;
    edit.lens.ca_red = 0.003;
    edit.lens.ca_blue = -0.002;
    assert!(!edit.lens.is_identity(false));
    edit
}

/// The base the CPU makes of `frame` under `edit` with the lens
/// switched off: a picture for the lens alone to move.
fn picture_before_lens(frame: &Arc<RawFrame>, edit: &Edit) -> Arc<WorkingImage> {
    let mut before = edit.clone();
    before.lens.enabled = false;
    let mut base = None;
    develop_turned(frame, &before, 0, &mut base);
    base.expect("a base").image
}

/// `image` as a picture that is not a raw: no CA correction runs on
/// it, so only the lens can put a base of it on the GPU.
fn picture_input(image: &Arc<WorkingImage>) -> Input {
    Input::Picture {
        image: image.clone(),
        meta: Arc::new(greycard_core::picture::Picture {
            image: WorkingImage::new(0, 0),
            make: String::new(),
            model: String::new(),
            shot: Default::default(),
            metadata: Default::default(),
            space: "sRGB".into(),
            bits: 16,
            white: greycard_core::illuminant::D65,
        }),
    }
}

/// `develop_job` on `input` under `edit`: the picture, when it came
/// back on the CPU.
fn develop_picture(
    input: &Input,
    edit: &Edit,
    base: &mut Option<FrameKept>,
    gpu: &mut Option<greycard_gpu::Context>,
) -> Option<Arc<WorkingImage>> {
    let deliver: Deliver = Arc::new(|_| {});
    let (outcome, image) = develop_job(
        input,
        edit,
        0,
        1,
        base,
        &mut Ai::new(),
        None,
        None,
        gpu,
        &deliver,
    );
    assert!(matches!(outcome, Outcome::Developed { .. }));
    image
}

/// The base's lens on the GPU is the reference's within the GPU op's
/// own tolerance (greycard-gpu's lens tests say why 1e-4 of a value in
/// 0 to 1), and a base it ran in says so, so the export makes its own.
/// On a picture that is not a raw, which has no CA correction, so only
/// the lens can have marked it; the sharpen off, so the develop ends
/// on the CPU.
#[test]
fn a_base_with_the_gpu_lens_is_the_cpu_s() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the GPU lens step");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let mut edit = lens_edit();
    edit.sharpen.enabled = false;
    let before = picture_before_lens(&frame, &edit);
    let input = picture_input(&before);
    let develop = |base: &mut Option<FrameKept>, gpu: &mut Option<greycard_gpu::Context>| {
        develop_picture(&input, &edit, base, gpu)
    };
    let mut gpu = Some(ctx);
    let mut on_gpu = None;
    develop(&mut on_gpu, &mut gpu);
    let mut on_cpu = None;
    develop(&mut on_cpu, &mut None);
    let (g, c) = (on_gpu.as_ref().unwrap(), on_cpu.as_ref().unwrap());
    assert!(g.on_gpu && !c.on_gpu);
    let lens = |s: &crate::stages::Stage| matches!(s, crate::stages::Stage::Lens(_));
    assert!(g.kept.ran_on_device(crate::stages::Keep::Base, lens));
    assert_ne!(c.image.data, before.data, "the lens moved something");
    let scale = c.image.data.iter().fold(1f32, |m, v| m.max(v.abs()));
    let worst = c
        .image
        .data
        .iter()
        .zip(&g.image.data)
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    assert!(worst <= 1e-4 * scale, "{worst} apart, values up to {scale}");

    // The export's develop, which has no GPU, makes the reference's
    // afresh from the base the GPU marked.
    let exported = develop(&mut on_gpu, &mut None).expect("a picture on the CPU");
    assert!(on_gpu.as_ref().is_some_and(|b| !b.on_gpu));
    let reference = develop(&mut None, &mut None).expect("a picture on the CPU");
    assert_eq!(exported.data, reference.data);
}

/// A device error in the lens falls back to the reference for that
/// base, the base the CPU's to the bit and not marked, and the develop
/// drops the context for the session, as it does after the CA's. A
/// picture the device cannot hold falls back the same way and is not
/// an error: the context stays. A device allowed 20 workgroups a
/// dimension cannot run the 400x320's 25 across; one whose textures
/// stop at 256 a side cannot hold it. On a picture that is not a raw,
/// where only the lens would run on the device, with the sharpen off,
/// so that no op but the lens asks the device for anything; without
/// the lens the same develop keeps it.
#[test]
fn a_gpu_error_in_the_lens_falls_back_and_an_unsupported_size_keeps_the_context() {
    use greycard_gpu::wgpu::Limits;
    let frame = Arc::new(aberrated_frame(400, 320));
    let mut lensed = lens_edit();
    lensed.sharpen.enabled = false;
    let input = picture_input(&picture_before_lens(&frame, &lensed));
    let mut cpu = None;
    develop_picture(&input, &lensed, &mut cpu, &mut None);
    let cpu = cpu.expect("a base");
    for (limits, device_error) in [
        (
            Limits {
                max_compute_workgroups_per_dimension: 20,
                ..Limits::default()
            },
            true,
        ),
        (
            Limits {
                max_texture_dimension_2d: 256,
                ..Limits::default()
            },
            false,
        ),
    ] {
        let Some(ctx) = context_with(limits.clone()) else {
            skipped("the lens fallback check");
            return;
        };
        let mut quiet = Edit::default();
        quiet.sharpen.enabled = false;
        let mut gpu = Some(ctx);
        develop_picture(&input, &quiet, &mut None, &mut gpu);
        assert!(gpu.is_some(), "{limits:?}");
        let mut base = None;
        develop_picture(&input, &lensed, &mut base, &mut gpu);
        let b = base.expect("a base");
        assert!(!b.on_gpu, "{limits:?}");
        assert_eq!(b.image.data, cpu.image.data, "{limits:?}");
        assert_eq!(gpu.is_none(), device_error, "{limits:?}");
    }
}

/// The Detail section on the GPU: a move runs the local contrast
/// there from the patched picture's upload, which is kept for the
/// next; the dehaze runs there after it, from the same upload, its
/// reduced copy kept for a move of the Dehaze alone and made again
/// when the local contrast moves; with the sharpen off the last op
/// writes the viewport's texture itself; with neither op on,
/// nothing is kept. And each path keeps on the device only what it
/// uses: the local contrast's planes those its sliders need (none
/// when they are made for each run, as on an integrated GPU), none
/// with the Detail section off, and the sharpen's none with the
/// sharpen off.
#[test]
fn the_detail_section_runs_on_the_gpu_from_the_kept_upload() {
    for keep in [true, false] {
        let Some(ctx) = greycard_gpu::Context::own().ok() else {
            skipped("the Detail section's GPU path");
            return;
        };
        ctx.keep_local_contrast_planes(keep);
        detail_section_on_the_gpu(ctx, keep);
    }
}

/// What a learned mask short of the card has the worker let go
/// ([`super::engine::card_release`]) is the context's working sets, both
/// emptied and their bytes said, and nothing of the open frame's: its
/// picture before the sharpen and its upload stay on the device, and
/// the next develop makes the sets again. The test's card figure takes
/// back what the release says it let go; that the driver's budget rises
/// is for a real card to show.
#[test]
fn a_masks_release_keeps_the_open_frames_device_points() {
    use greycard_ai::{Provider, Store};
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("a learned mask's release");
        return;
    };
    ctx.keep_local_contrast_planes(true);
    let frame = Arc::new(aberrated_frame(400, 320));
    let mut gpu = Some(ctx);
    let mut base = None;
    let mut edit = Edit::default();
    edit.detail.clarity = 0.5;
    let (outcome, _) = develop_with(&frame, &edit, &mut base, &mut gpu);
    assert!(matches!(
        outcome,
        Outcome::Developed {
            image: Developed::Texture(_),
            detail: Some((_, DetailRan::Gpu)),
            sharpen: Some(_),
            ..
        }
    ));
    let held = gpu.as_ref().unwrap().kept_bytes();
    assert!(held.sharpen > 0 && held.local_contrast > 0, "{held:?}");
    let upload = base.as_ref().unwrap().kept.upload().unwrap().clone();
    // Short by a byte, until the sets go.
    let short = crate::ai::mask_needs(crate::ai::MaskModel::People) - 1;
    let mut ai = Ai::for_test(Store::at("no-store"), vec![Provider::WebGpu, Provider::Cpu]);
    ai.free_memory_by_test = Some(short);
    ai.set_cpu_fallback(true);
    ai.set_card_release(gpu.as_ref().map(super::engine::card_release));
    ai.mask_room(crate::ai::MaskModel::People)
        .expect("not refused");
    assert!(!ai.masks_on_cpu(), "the card's once the sets went");
    assert_eq!(
        ai.free_memory_by_test,
        Some(short + held.sharpen + held.local_contrast)
    );
    assert_eq!(
        gpu.as_ref().unwrap().kept_bytes(),
        greycard_gpu::KeptBytes::default()
    );
    let b = base.as_ref().unwrap();
    assert!(b.kept.on_device(Keep::PreSharpen), "the open frame's point");
    assert!(Arc::ptr_eq(b.kept.upload().unwrap(), &upload));
    // The next develop makes the sets again.
    edit.detail.clarity = -0.5;
    let (outcome, _) = develop_with(&frame, &edit, &mut base, &mut gpu);
    assert!(matches!(
        outcome,
        Outcome::Developed {
            image: Developed::Texture(_),
            detail: Some((_, DetailRan::Gpu)),
            ..
        }
    ));
    assert_eq!(
        gpu.as_ref().unwrap().kept_bytes().local_contrast,
        held.local_contrast
    );
}

fn detail_section_on_the_gpu(ctx: greycard_gpu::Context, keep: bool) {
    let frame = Arc::new(aberrated_frame(400, 320));
    let deliver: Deliver = Arc::new(|_| {});
    let mut gpu = Some(ctx);
    let mut base = None;
    let develop = |edit: &Edit, base: &mut Option<FrameKept>, gpu: &mut Option<_>| {
        develop_job(
            &Input::Raw(frame.clone()),
            edit,
            0,
            1,
            base,
            &mut Ai::new(),
            None,
            None,
            gpu,
            &deliver,
        )
    };
    let kept = |gpu: &Option<greycard_gpu::Context>| gpu.as_ref().unwrap().kept_bytes();
    let mut edit = Edit::default();
    edit.detail.clarity = 0.5;
    let (outcome, image) = develop(&edit, &mut base, &mut gpu);
    assert!(
        matches!(
            outcome,
            Outcome::Developed {
                image: Developed::Texture(_),
                detail: Some((_, DetailRan::Gpu)),
                sharpen: Some(_),
                ..
            }
        ),
        "the local contrast and the sharpen on the GPU"
    );
    assert!(image.is_none());
    let b = base.as_ref().unwrap();
    let uploaded = b
        .kept
        .upload()
        .expect("the patched picture is kept")
        .clone();
    assert!(b.kept.on_device(Keep::PreSharpen));
    // A plane of the picture's size; Clarity's exact filter at this
    // size takes the log, its scratch, and the slope and intercept.
    let plane = u64::from(uploaded.width()) * u64::from(uploaded.height()) * 4;
    let planes = |n: u64| if keep { n * plane } else { 0 };
    assert!(kept(&gpu).sharpen > 0);
    assert_eq!(kept(&gpu).local_contrast, planes(4), "Clarity's planes");
    // Another move keeps the upload and remakes the picture
    // before the sharpen from it.
    edit.detail.clarity = -0.5;
    let (outcome, _) = develop(&edit, &mut base, &mut gpu);
    assert!(matches!(
        outcome,
        Outcome::Developed {
            image: Developed::Texture(_),
            detail: Some((_, DetailRan::Gpu)),
            ..
        }
    ));
    let b = base.as_ref().unwrap();
    assert!(Arc::ptr_eq(b.kept.upload().unwrap(), &uploaded));
    assert!(b.kept.on_device(Keep::PreSharpen));
    // A sharpen move keeps the picture before it, and says so.
    edit.sharpen.iterations += 1;
    let (outcome, _) = develop(&edit, &mut base, &mut gpu);
    assert!(matches!(
        outcome,
        Outcome::Developed {
            detail: Some((_, DetailRan::Kept)),
            sharpen: Some(_),
            ..
        }
    ));
    // Texture as well: its gain beside them.
    edit.detail.texture = 0.5;
    develop(&edit, &mut base, &mut gpu);
    assert_eq!(kept(&gpu).local_contrast, planes(5), "Texture's planes too");
    // The Detail section off, the sharpen on: the sharpen reads the
    // upload itself, and the local contrast keeps nothing.
    edit.detail.texture = 0.0;
    edit.detail.clarity = 0.0;
    develop(&edit, &mut base, &mut gpu);
    let b = base.as_ref().unwrap();
    assert!(Arc::ptr_eq(b.kept.upload().unwrap(), &uploaded));
    assert_eq!(kept(&gpu).local_contrast, 0, "no Detail, no planes");
    // The dehaze after the local contrast, both on the GPU from the
    // kept upload; its reduced copy kept with what it was made from.
    edit.detail.clarity = -0.5;
    develop(&edit, &mut base, &mut gpu);
    assert_eq!(kept(&gpu).local_contrast, planes(4));
    assert!(!base.as_ref().unwrap().kept.has_haze());
    edit.detail.dehaze = 0.5;
    // The dehaze goes on from the local contrast's picture kept on
    // the device, which a slider after it left as it was.
    let (outcome, _) = develop(&edit, &mut base, &mut gpu);
    let Outcome::Developed {
        image: Developed::Texture(_),
        detail: Some((_, DetailRan::Kept)),
        dehaze: Some(first),
        ..
    } = outcome
    else {
        panic!("the dehaze on the GPU after the kept local contrast");
    };
    let b = base.as_ref().unwrap();
    assert!(Arc::ptr_eq(b.kept.upload().unwrap(), &uploaded));
    assert!(b.kept.on_device(Keep::PreSharpen));
    let reduced = b
        .kept
        .haze_copy()
        .expect("the reduced copy is kept")
        .clone();
    assert_eq!(kept(&gpu).local_contrast, planes(4));
    assert!(kept(&gpu).sharpen > 0);
    // A move of the Dehaze alone fits again from the kept copy, so
    // the airlight is the same.
    edit.detail.dehaze = 0.8;
    let (outcome, _) = develop(&edit, &mut base, &mut gpu);
    let Outcome::Developed {
        dehaze: Some(second),
        ..
    } = outcome
    else {
        panic!("a Dehaze move");
    };
    let b = base.as_ref().unwrap();
    assert!(Arc::ptr_eq(b.kept.haze_copy().unwrap(), &reduced));
    assert_eq!(second.airlight, first.airlight);
    assert!(second.strength > first.strength);
    // A sharpen move keeps the picture before it, dehaze and all.
    edit.sharpen.iterations += 1;
    let (outcome, _) = develop(&edit, &mut base, &mut gpu);
    assert!(matches!(
        outcome,
        Outcome::Developed {
            detail: Some((_, DetailRan::Kept)),
            dehaze: Some(_),
            ..
        }
    ));
    // A move of the local contrast makes the copy again.
    edit.detail.clarity = -0.3;
    develop(&edit, &mut base, &mut gpu);
    let b = base.as_ref().unwrap();
    assert!(!Arc::ptr_eq(b.kept.haze_copy().unwrap(), &reduced));
    // The dehaze alone: from the upload, the local contrast's planes
    // let go.
    edit.detail.clarity = 0.0;
    let (outcome, _) = develop(&edit, &mut base, &mut gpu);
    assert!(matches!(
        outcome,
        Outcome::Developed {
            image: Developed::Texture(_),
            detail: None,
            dehaze: Some(_),
            ..
        }
    ));
    assert!(base.as_ref().unwrap().kept.has_haze());
    assert_eq!(kept(&gpu).local_contrast, 0, "no local contrast, no planes");
    // With the sharpen off, the dehaze writes the viewport's
    // texture, after the local contrast or alone.
    edit.sharpen.enabled = false;
    for clarity in [0.4, 0.0] {
        edit.detail.clarity = clarity;
        let (outcome, image) = develop(&edit, &mut base, &mut gpu);
        assert!(
            matches!(
                outcome,
                Outcome::Developed {
                    image: Developed::Texture(_),
                    sharpen: None,
                    dehaze: Some(_),
                    ..
                }
            ),
            "the dehaze into the viewport's texture"
        );
        assert!(image.is_none());
        let b = base.as_ref().unwrap();
        assert!(!b.kept.on_device(Keep::PreSharpen) && b.kept.has_haze());
        assert_eq!(kept(&gpu).sharpen, 0);
    }
    edit.sharpen.enabled = true;
    edit.detail.clarity = -0.5;
    // Sharpen off, dehaze off: the op writes the viewport's texture
    // itself, and nothing waits for a sharpen: its planes go.
    edit.detail.dehaze = 0.0;
    edit.sharpen.enabled = false;
    let (outcome, image) = develop(&edit, &mut base, &mut gpu);
    assert!(
        matches!(
            outcome,
            Outcome::Developed {
                image: Developed::Texture(_),
                detail: Some((_, DetailRan::Gpu)),
                sharpen: None,
                ..
            }
        ),
        "the local contrast alone, on the GPU"
    );
    assert!(image.is_none());
    let b = base.as_ref().unwrap();
    assert!(b.kept.has_upload() && !b.kept.on_device(Keep::PreSharpen));
    assert_eq!(kept(&gpu).sharpen, 0, "the sharpen's planes are let go");
    assert_eq!(kept(&gpu).local_contrast, planes(4));
    // Neither op: the CPU's halves, and nothing kept on the device.
    edit.detail.clarity = 0.0;
    let (outcome, image) = develop(&edit, &mut base, &mut gpu);
    assert!(matches!(
        outcome,
        Outcome::Developed {
            image: Developed::Halves(_),
            detail: None,
            sharpen: None,
            ..
        }
    ));
    assert!(image.is_some());
    let b = base.as_ref().unwrap();
    assert!(!b.kept.has_upload() && !b.kept.on_device(Keep::PreSharpen));
    assert_eq!(kept(&gpu), greycard_gpu::KeptBytes::default());
    assert!(gpu.is_some(), "the context is kept throughout");
}

/// The Detail section on the GPU gives the picture the CPU's does:
/// the local contrast, then the dehaze, in the reference's order,
/// with the sharpen off so that the viewport's texture is the
/// Detail section's output alone. From one base made on the CPU,
/// so that the CA correction is the same on both. The two differ by
/// the viewport's half floats, the local contrast's rounding and
/// the dehaze's gain on it; the dehaze run before the local
/// contrast would be percents off.
#[test]
fn the_detail_section_on_the_gpu_is_the_cpus() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the Detail section's GPU picture");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let deliver: Deliver = Arc::new(|_| {});
    let mut gpu = Some(ctx);
    let mut base = None;
    let mut edit = Edit::default();
    edit.sharpen.enabled = false;
    for (texture, clarity, dehaze) in [(0.4, 0.6, 0.7), (0.0, 0.0, -0.8), (-0.5, 0.3, 1.0)] {
        edit.detail.texture = texture;
        edit.detail.clarity = clarity;
        edit.detail.dehaze = dehaze;
        let mut develop = |gpu: &mut Option<greycard_gpu::Context>| {
            develop_job(
                &Input::Raw(frame.clone()),
                &edit,
                0,
                1,
                &mut base,
                &mut Ai::new(),
                None,
                None,
                gpu,
                &deliver,
            )
        };
        let (cpu_outcome, cpu) = develop(&mut None);
        let (outcome, _) = develop(&mut gpu);
        let (
            Outcome::Developed {
                dehaze: Some(cpu_stats),
                ..
            },
            Outcome::Developed {
                image: Developed::Texture(out),
                dehaze: Some(gpu_stats),
                ..
            },
        ) = (cpu_outcome, outcome)
        else {
            panic!("both develops, the GPU's into a texture");
        };
        let cpu = cpu.unwrap();
        let viewport = read_viewport(gpu.as_ref().unwrap(), &out);
        let worst = cpu
            .data
            .iter()
            .zip(&viewport.data)
            .map(|(a, b)| (a - b).abs() / a.abs().max(0.05))
            .fold(0.0, f32::max);
        println!(
            "texture {texture}, clarity {clarity}, dehaze {dehaze}: worst {worst:.2e}; airlight {:?} / {:?}",
            cpu_stats.airlight, gpu_stats.airlight
        );
        // One of a half float's ulps: half of one for the rounding to
        // nearest, and the ops' own differences inside the rest.
        assert!(
            worst < 1e-3,
            "the viewport is {worst:.2e} off the CPU's picture"
        );
        for c in 0..3 {
            assert!((cpu_stats.airlight[c] - gpu_stats.airlight[c]).abs() < 1e-4);
        }
        assert!((cpu_stats.transmission_mean - gpu_stats.transmission_mean).abs() < 1e-4);
    }
}

/// A GPU error in the dehaze is warned once and takes the session
/// to the CPU, as the other ops' do: here a device whose storage
/// buffers are too small for the dehaze's block sums, which its
/// error scope catches. The develop that met it is the CPU's, to
/// the bit.
#[test]
fn a_gpu_error_in_the_dehaze_takes_the_session_to_the_cpu() {
    let Some(ctx) = context_with(greycard_gpu::wgpu::Limits {
        max_storage_buffer_binding_size: 256,
        ..greycard_gpu::wgpu::Limits::default()
    }) else {
        skipped("the dehaze's device error");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let deliver: Deliver = Arc::new(|_| {});
    let mut edit = Edit::default();
    edit.detail.dehaze = 0.6;
    for sharpen in [true, false] {
        edit.sharpen.enabled = sharpen;
        let mut gpu = Some(
            context_with(greycard_gpu::wgpu::Limits {
                max_storage_buffer_binding_size: 256,
                ..greycard_gpu::wgpu::Limits::default()
            })
            .unwrap(),
        );
        let mut base = None;
        let mut develop = |gpu: &mut Option<greycard_gpu::Context>| {
            develop_job(
                &Input::Raw(frame.clone()),
                &edit,
                0,
                1,
                &mut base,
                &mut Ai::new(),
                None,
                None,
                gpu,
                &deliver,
            )
        };
        let (_, cpu) = develop(&mut None);
        let (outcome, image) = develop(&mut gpu);
        assert!(gpu.is_none(), "the context is dropped after a device error");
        assert!(matches!(
            outcome,
            Outcome::Developed {
                image: Developed::Halves(_),
                dehaze: Some(_),
                ..
            }
        ));
        assert_eq!(image.unwrap().data, cpu.unwrap().data);
    }
    drop(ctx);
}

/// An export of a picture larger than the device's textures, wider
/// than them or taller, is the full picture, and the export a run
/// without a device makes, to the bit: every op the device cannot
/// hold it for runs on the CPU, nothing made of its size reaches the
/// device (whose uncaptured handler panics), and the context is kept.
#[test]
fn an_export_past_the_devices_textures_is_the_cpus_at_full_size() {
    use greycard_render::export::Pixels;
    let mut edit = Edit::default();
    edit.sharpen.enabled = true;
    edit.detail.clarity = 0.4;
    edit.detail.dehaze = 0.3;
    let settings = greycard_render::export::Settings::default();
    for (w, h) in [(400usize, 120usize), (120, 400)] {
        let Some(ctx) = context_with(greycard_gpu::wgpu::Limits {
            max_texture_dimension_2d: 256,
            ..greycard_gpu::wgpu::Limits::default()
        }) else {
            skipped("the export past the device's textures");
            return;
        };
        let input = Input::Raw(Arc::new(aberrated_frame(w, h)));
        let deliver: Deliver = Arc::new(|_| {});
        let mut ai = Ai::new();
        let mut export = |gpu: &mut Option<greycard_gpu::Context>| {
            super::export::render_open(
                &edit,
                0,
                &input,
                None,
                &mut WorkerModels::new(&mut ai, None, &deliver, 0),
                None,
                gpu,
                &settings,
            )
            .expect("exported")
            .0
        };
        let mut gpu = Some(ctx);
        let on_device = export(&mut gpu);
        assert!(gpu.is_some(), "{w}x{h}: the context is kept");
        let reference = export(&mut None);
        assert_eq!(
            (on_device.width, on_device.height),
            (reference.width, reference.height),
            "{w}x{h}"
        );
        assert!(
            on_device.width.max(on_device.height) > 256,
            "{w}x{h}: the full picture, past the device"
        );
        let same = match (&on_device.pixels, &reference.pixels) {
            (Pixels::Eight(a), Pixels::Eight(b)) => a == b,
            (Pixels::Sixteen(a), Pixels::Sixteen(b)) => a == b,
            _ => false,
        };
        assert!(same, "{w}x{h}: the export is the CPU's");
    }
}

/// A picture larger than the device's textures, with a Detail
/// slider set, is the CPU's for that develop, with the sharpen off
/// and on, and the context is kept: the next picture that fits is
/// the GPU's again. No texture is made for it before the upload
/// has found it fits, so nothing reaches wgpu's uncaptured handler
/// (which panics the worker).
#[test]
fn a_picture_the_device_cannot_hold_is_the_cpus_for_that_develop() {
    let Some(ctx) = context_with(greycard_gpu::wgpu::Limits {
        max_texture_dimension_2d: 256,
        ..greycard_gpu::wgpu::Limits::default()
    }) else {
        skipped("the oversized picture's develop");
        return;
    };
    let deliver: Deliver = Arc::new(|_| {});
    let mut gpu = Some(ctx);
    let large = Arc::new(aberrated_frame(400, 320));
    let small = Arc::new(aberrated_frame(200, 160));
    for sharpen in [false, true] {
        for dehaze in [0.0, 0.6] {
            let mut edit = Edit::default();
            edit.sharpen.enabled = sharpen;
            edit.detail.clarity = 0.5;
            edit.detail.dehaze = dehaze;
            let develop = |frame: &Arc<RawFrame>,
                           base: &mut Option<FrameKept>,
                           gpu: &mut Option<greycard_gpu::Context>| {
                develop_job(
                    &Input::Raw(frame.clone()),
                    &edit,
                    0,
                    1,
                    base,
                    &mut Ai::new(),
                    None,
                    None,
                    gpu,
                    &deliver,
                )
            };
            let what = format!("sharpen {sharpen}, dehaze {dehaze}");
            let mut base = None;
            let (outcome, image) = develop(&large, &mut base, &mut gpu);
            assert!(
                matches!(
                    outcome,
                    Outcome::Developed {
                        image: Developed::Halves(_),
                        detail: Some((_, DetailRan::Cpu(_))),
                        ..
                    }
                ),
                "{what}: the CPU's develop"
            );
            assert!(gpu.is_some(), "{what}: the context is kept");
            let (_, reference) = develop(&large, &mut None, &mut None);
            assert_eq!(image.unwrap().data, reference.unwrap().data, "{what}");
            let b = base.as_ref().unwrap();
            assert!(
                !b.kept.has_upload() && !b.kept.on_device(Keep::PreSharpen) && !b.kept.has_haze()
            );
            // Nor the CPU's picture before the sharpen, the size of
            // the frame, kept on the frames largest.
            assert!(!b.kept.holds(Keep::PreSharpen), "{what}");
            let (outcome, _) = develop(&small, &mut None, &mut gpu);
            assert!(
                matches!(
                    outcome,
                    Outcome::Developed {
                        image: Developed::Texture(_),
                        detail: Some((_, DetailRan::Gpu)),
                        ..
                    }
                ),
                "{what}: the next picture that fits is the GPU's"
            );
        }
    }
}

/// A move of the sharpen alone goes on from the picture before it,
/// kept on the device, and gives the texture a develop made afresh
/// gives, to the bit; so does a move of the light, which goes on from
/// the same point. A move of the Detail section goes back to the
/// patched picture's upload.
#[test]
fn a_sharpen_move_from_the_kept_picture_is_a_fresh_develop() {
    use crate::stages::Keep;
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the sharpen move's picture");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let deliver: Deliver = Arc::new(|_| {});
    let mut gpu = Some(ctx);
    let develop =
        |edit: &Edit, base: &mut Option<FrameKept>, gpu: &mut Option<greycard_gpu::Context>| {
            let (outcome, _) = develop_job(
                &Input::Raw(frame.clone()),
                edit,
                0,
                1,
                base,
                &mut Ai::new(),
                None,
                None,
                gpu,
                &deliver,
            );
            let Outcome::Developed {
                image: Developed::Texture(texture),
                detail: Some((_, ran)),
                sharpen: Some(_),
                ..
            } = outcome
            else {
                panic!("the local contrast and the sharpen on the GPU, into a texture");
            };
            (texture, ran)
        };
    let mut edit = Edit::default();
    edit.detail.clarity = 0.5;
    let mut base = None;
    let (_, ran) = develop(&edit, &mut base, &mut gpu);
    assert_eq!(ran, DetailRan::Gpu);
    let b = base.as_ref().unwrap();
    assert!(b.kept.on_device(Keep::PreSharpen) && b.kept.has_upload());
    let mut moves = Vec::new();
    edit.sharpen.iterations += 1;
    moves.push((edit.clone(), DetailRan::Kept));
    edit.light.exposure = 0.7;
    moves.push((edit.clone(), DetailRan::Kept));
    edit.detail.clarity = 0.2;
    moves.push((edit.clone(), DetailRan::Gpu));
    for (edit, expected) in moves {
        let (moved, ran) = develop(&edit, &mut base, &mut gpu);
        assert_eq!(ran, expected);
        let (fresh, _) = develop(&edit, &mut None, &mut gpu);
        let ctx = gpu.as_ref().unwrap();
        let (moved, fresh) = (read_viewport(ctx, &moved), read_viewport(ctx, &fresh));
        assert!(
            moved.data == fresh.data,
            "{expected:?}: the move's picture is not a fresh develop's"
        );
    }
}

/// A mask's Detail reads bands made beside the run on the device,
/// which land after the run's keeps are put: a sharpen move, and a
/// Dehaze move that goes on from the local contrast kept on the device,
/// still hand the window the bands, so a mask's Texture does not go.
#[test]
fn a_masks_detail_bands_outlive_a_move_after_them() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the masks' bands on a move after them");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let deliver: Deliver = Arc::new(|_| {});
    let mut gpu = Some(ctx);
    let mut edit = Edit::default();
    edit.detail.clarity = 0.4;
    let mut a = greycard_edit::Adjustment {
        id: 1,
        ..Default::default()
    };
    a.mask
        .components
        .push(greycard_edit::mask::Component::default());
    a.look.detail.texture = -0.5;
    edit.adjustments.push(a);
    let mut base = None;
    let mut develop = |edit: &Edit| {
        let (outcome, _) = develop_job(
            &Input::Raw(frame.clone()),
            edit,
            0,
            1,
            &mut base,
            &mut Ai::new(),
            None,
            None,
            &mut gpu,
            &deliver,
        );
        let Outcome::Developed {
            planes,
            detail: Some((_, ran)),
            ..
        } = outcome
        else {
            panic!("developed, with the local contrast");
        };
        (planes.is_some(), ran)
    };
    assert_eq!(develop(&edit), (true, DetailRan::Gpu));
    edit.sharpen.iterations += 1;
    assert_eq!(develop(&edit), (true, DetailRan::Kept), "a sharpen move");
    edit.detail.dehaze = 0.5;
    assert_eq!(develop(&edit), (true, DetailRan::Kept), "a Dehaze move");
}

/// A Dehaze set with the sharpen on goes on from the local contrast's
/// picture kept on the device (one moved runs the local contrast
/// again, there being no keep between the two), and gives the texture a develop made
/// afresh gives, to the bit.
#[test]
fn a_dehaze_from_the_kept_local_contrast_is_a_fresh_develop() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the Dehaze after the kept local contrast");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let deliver: Deliver = Arc::new(|_| {});
    let mut gpu = Some(ctx);
    let develop =
        |edit: &Edit, base: &mut Option<FrameKept>, gpu: &mut Option<greycard_gpu::Context>| {
            let (outcome, _) = develop_job(
                &Input::Raw(frame.clone()),
                edit,
                0,
                1,
                base,
                &mut Ai::new(),
                None,
                None,
                gpu,
                &deliver,
            );
            let Outcome::Developed {
                image: Developed::Texture(texture),
                detail: Some((_, ran)),
                sharpen: Some(_),
                ..
            } = outcome
            else {
                panic!("the Detail section and the sharpen on the GPU, into a texture");
            };
            (texture, ran)
        };
    let mut edit = Edit::default();
    edit.detail.clarity = 0.5;
    let mut base = None;
    for dehaze in [0.4, 0.9] {
        edit.detail.dehaze = 0.0;
        develop(&edit, &mut base, &mut gpu);
        edit.detail.dehaze = dehaze;
        let (moved, ran) = develop(&edit, &mut base, &mut gpu);
        assert_eq!(ran, DetailRan::Kept, "{dehaze}");
        let (fresh, _) = develop(&edit, &mut None, &mut gpu);
        let ctx = gpu.as_ref().unwrap();
        let (moved, fresh) = (read_viewport(ctx, &moved), read_viewport(ctx, &fresh));
        assert!(
            moved.data == fresh.data,
            "Dehaze {dehaze}: the move's picture is not a fresh develop's"
        );
    }
}

/// A walk nobody waits on (a prefetch) that meets a device error short
/// of a lost device goes on on the CPU and leaves the session its
/// context; the window's walk under the same error lets it go. Either
/// way the picture is the CPU's. A device allowed 20 workgroups a
/// dimension cannot run the 400x320's CA.
#[test]
fn a_prefetch_s_device_error_leaves_the_session_its_device() {
    use crate::stages::{Purpose, stages};
    use crate::walk::{Buf, Hold, Kept, NoHold, NoModels, plan, walk};
    struct Speculative;
    impl Hold for Speculative {
        fn keeps_device(&self) -> bool {
            true
        }
    }
    let limits = greycard_gpu::wgpu::Limits {
        max_compute_workgroups_per_dimension: 20,
        ..greycard_gpu::wgpu::Limits::default()
    };
    let Some(ctx) = context_with(limits.clone()) else {
        skipped("the prefetch's device error");
        return;
    };
    let input = crate::walk::Input::Raw(Arc::new(aberrated_frame(400, 320)));
    let purpose = Purpose::Prefetch;
    let steps = stages(&Edit::default(), &purpose, &input.facts(0), None);
    let picture = |hold: &dyn Hold, gpu: &mut Option<greycard_gpu::Context>| {
        let places = plan(&steps, &purpose, gpu.is_some());
        let walked = walk(
            &steps,
            &places,
            &input,
            &mut Kept::new(),
            &mut NoModels,
            gpu,
            hold,
        )
        .expect("walks");
        match walked.buf {
            Buf::Working(image) => image,
            _ => panic!("a prefetch ends at a picture in memory"),
        }
    };
    let reference = picture(&NoHold, &mut None);
    let mut gpu = Some(ctx);
    let fetched = picture(&Speculative, &mut gpu);
    assert!(gpu.is_some(), "the session keeps its device");
    assert!(fetched.data == reference.data);
    let mut gpu = context_with(limits);
    let window = picture(&NoHold, &mut gpu);
    assert!(gpu.is_none(), "the window's walk lets it go");
    assert!(window.data == reference.data);
}

/// An export whose CA meets a device error short of a lost device
/// (a device allowed 20 workgroups a dimension cannot run the
/// 400x320's) writes that frame on the CPU and leaves the session its
/// context, as a prefetch does, so one large file never puts the
/// viewport on the CPU; the file is the one an export with no device
/// writes.
#[test]
fn an_export_s_device_error_leaves_the_session_its_device() {
    let limits = greycard_gpu::wgpu::Limits {
        max_compute_workgroups_per_dimension: 20,
        ..greycard_gpu::wgpu::Limits::default()
    };
    let Some(ctx) = context_with(limits) else {
        skipped("the export's device error");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let settings = greycard_render::export::Settings {
        format: greycard_render::export::Format::Tiff,
        ..Default::default()
    };
    let deliver: Deliver = Arc::new(|_| {});
    let export = |gpu: &mut Option<greycard_gpu::Context>| {
        let mut ai = Ai::new();
        super::export::walk_export(
            Input::Raw(frame.clone()),
            &Edit::default(),
            0,
            &mut WorkerModels::new(&mut ai, None, &deliver, 0),
            None,
            gpu,
            &settings,
        )
        .expect("written")
        .0
        .pixels
    };
    let sixteen = |p: greycard_render::export::Pixels| match p {
        greycard_render::export::Pixels::Sixteen(v) => v,
        greycard_render::export::Pixels::Eight(_) => panic!("a TIFF's pixels are sixteen bits"),
    };
    let mut gpu = Some(ctx);
    let on_device = sixteen(export(&mut gpu));
    assert!(gpu.is_some(), "the session keeps its device");
    let reference = sixteen(export(&mut None));
    assert!(on_device == reference);
}

/// A move of the Dehaze alone, which fits again from the kept
/// reduced copy, gives the picture a develop made afresh at that
/// amount gives, to the bit: with the sharpen off (the dehaze's own
/// output in the viewport's texture) and on.
#[test]
fn a_dehaze_move_from_the_kept_copy_is_a_fresh_develop() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the Dehaze move's picture");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let deliver: Deliver = Arc::new(|_| {});
    let mut gpu = Some(ctx);
    for sharpen in [false, true] {
        let mut edit = Edit::default();
        edit.sharpen.enabled = sharpen;
        edit.detail.texture = 0.3;
        edit.detail.clarity = 0.5;
        let mut develop = |edit: &Edit, base: &mut Option<FrameKept>| {
            let (outcome, _) = develop_job(
                &Input::Raw(frame.clone()),
                edit,
                0,
                1,
                base,
                &mut Ai::new(),
                None,
                None,
                &mut gpu,
                &deliver,
            );
            let Outcome::Developed {
                image: Developed::Texture(texture),
                dehaze: Some(_),
                ..
            } = outcome
            else {
                panic!("the dehaze on the GPU, into a texture");
            };
            texture
        };
        let mut base = None;
        edit.detail.dehaze = 0.4;
        develop(&edit, &mut base);
        let reduced = base.as_ref().unwrap().kept.haze_copy().unwrap().clone();
        edit.detail.dehaze = 0.9;
        let moved = develop(&edit, &mut base);
        let kept = base.as_ref().unwrap().kept.haze_copy().unwrap();
        assert!(
            Arc::ptr_eq(kept, &reduced),
            "the move fitted from the kept copy"
        );
        let fresh = develop(&edit, &mut None);
        let ctx = gpu.as_ref().unwrap();
        let (moved, fresh) = (read_viewport(ctx, &moved), read_viewport(ctx, &fresh));
        assert!(
            moved.data == fresh.data,
            "sharpen {sharpen}: the move's picture is not a fresh develop's"
        );
    }
}

pub(super) fn thumb_scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-thumbjob-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cache_at(dir: &std::path::Path) -> ThumbCache {
    Arc::new(Mutex::new(Some(Thumbs::at(
        dir.join("thumbs"),
        greycard_library::thumbs::DEFAULT_CAP,
    ))))
}

/// A picture file's thumbnail kept before its profile was read is a
/// miss, made again and kept under the pictures' recipe; a raw's kept
/// under the same recipe is still a hit, its thumbnail unchanged.
#[test]
fn a_pictures_thumbnail_kept_before_profiles_were_read_is_made_again() {
    let dir = thumb_scratch("picture-recipe");
    let png = dir.join("exported.png");
    image::RgbImage::from_pixel(30, 20, image::Rgb([40, 190, 50]))
        .save(&png)
        .unwrap();
    let raw = dir.join("IMG_0003.CR3");
    std::fs::write(&raw, vec![0x33u8; 90_000]).unwrap();
    let cache = cache_at(&dir);
    let stale = Thumb {
        width: 3,
        height: 2,
        rgb: vec![7; 18],
    };
    for path in [&png, &raw] {
        let kept_before = Tag {
            recipe: THUMB_RECIPE,
            stamp: file_stat(path).unwrap().1,
        };
        let hash = greycard_library::hash_file(path).unwrap();
        cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .put(&hash, THUMB_WIDTH, kept_before, &stale)
            .unwrap();
    }
    assert_ne!(thumb_recipe(&png), THUMB_RECIPE);
    assert_eq!(thumb_recipe(&raw), THUMB_RECIPE);
    let look = |path: &std::path::Path| {
        cached_thumbnail_noting(
            &cache,
            None,
            None,
            &Frame::file(path),
            THUMB_WIDTH,
            thumbnail,
        )
        .unwrap()
    };
    let (made, cached) = look(&png);
    assert!(!cached, "the picture's old entry is a miss");
    assert_eq!((made.width, made.height), (30, 20));
    let (again, cached) = look(&png);
    assert!(cached, "and the new one is kept");
    assert_eq!(again.width, 30);
    let (kept, cached) = look(&raw);
    assert!(cached, "the raw's entry is still a hit");
    assert_eq!(kept.rgb, stale.rgb);
    crate::testing::remove_dir_retry(&dir);
}

/// A raw nothing can decode: its thumbnail can only have come
/// from the cache.
#[test]
fn a_cached_thumbnail_is_not_made_again() {
    let dir = thumb_scratch("hit");
    let raw = dir.join("IMG_0001.CR3");
    std::fs::write(&raw, vec![0x17u8; 90_000]).unwrap();
    let cache = cache_at(&dir);
    assert!(
        cached_thumbnail_noting(
            &cache,
            None,
            None,
            &Frame::file(&raw),
            THUMB_WIDTH,
            thumbnail
        )
        .is_err(),
        "not a raw, and nothing cached"
    );
    let kept = Thumb {
        width: 3,
        height: 2,
        rgb: vec![128; 18],
    };
    let hash = greycard_library::hash_file(&raw).unwrap();
    cache
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .put(
            &hash,
            THUMB_WIDTH,
            thumb_tag(&raw, file_stat(&raw).unwrap()),
            &kept,
        )
        .unwrap();
    // Renamed into another folder: still the cache's.
    let moved_dir = dir.join("renamed");
    std::fs::create_dir_all(&moved_dir).unwrap();
    let moved = moved_dir.join("wedding-0001.CR3");
    std::fs::rename(&raw, &moved).unwrap();
    let (thumb, cached) = cached_thumbnail_noting(
        &cache,
        None,
        None,
        &Frame::file(&moved),
        THUMB_WIDTH,
        thumbnail,
    )
    .unwrap();
    assert!(cached);
    assert_eq!((thumb.width, thumb.height), (3, 2));
    // Another size is not the same entry.
    assert!(
        cached_thumbnail_noting(&cache, None, None, &Frame::file(&moved), 256, thumbnail).is_err()
    );
    crate::testing::remove_dir_retry(&dir);
}

/// An offline frame's picture: looked up by the row's hash and
/// stamp at the size the pool keeps pictures at, the file itself
/// gone.
#[test]
fn a_cached_thumbnail_job_finds_what_the_pool_kept() {
    let dir = thumb_scratch("offline");
    let raw = dir.join("IMG_0002.CR3");
    std::fs::write(&raw, vec![0x2au8; 90_000]).unwrap();
    let hash = greycard_library::hash_file(&raw).unwrap();
    let stamp = thumb_tag(&raw, file_stat(&raw).unwrap()).stamp;
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        let line = match o {
            Outcome::Thumbnail {
                index,
                size,
                width,
                cached,
                ..
            } => format!("thumbnail {index} {size} {width} {cached}"),
            Outcome::NoThumbnail { index, .. } => format!("none {index}"),
            _ => return,
        };
        // A send after the test gave up must not panic on the worker's
        // thread: that aborts the whole run and hides which wait failed.
        let _ = tx.lock().unwrap().send(line);
    });
    let mut thumbs = Thumbs::at(dir.join("thumbs"), greycard_library::thumbs::DEFAULT_CAP);
    let made = crate::made::made_size(worker.pool().expect("a window's worker").size());
    let kept = Thumb {
        width: 5,
        height: 4,
        rgb: vec![90; 60],
    };
    thumbs
        .put(
            &hash,
            made,
            thumb_tag(&raw, file_stat(&raw).unwrap()),
            &kept,
        )
        .unwrap();
    worker.set_thumb_cache(Some(thumbs));
    std::fs::remove_file(&raw).unwrap();
    worker.send(Job::CachedThumbnail {
        index: 3,
        frame: Frame::file(raw.clone()),
        hash: hash.clone(),
        stamp,
    });
    // Ten seconds was too short on a loaded runner, beside the long
    // tests of this suite.
    let wait = std::time::Duration::from_secs(60);
    assert_eq!(
        rx.recv_timeout(wait).unwrap(),
        format!("thumbnail 3 {made} 5 true")
    );
    // Another stamp is another entry: a miss, and nothing made.
    worker.send(Job::CachedThumbnail {
        index: 4,
        frame: Frame::file(raw),
        hash,
        stamp: stamp + 1,
    });
    assert_eq!(rx.recv_timeout(wait).unwrap(), "none 4");
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// An offline picture's thumbnail kept before its profile was read,
/// under the raws' recipe, stands in while its file is out of reach;
/// with the file in reach, the job finds none and it is made again.
#[test]
fn a_cached_thumbnail_job_finds_a_pictures_earlier_entry_offline_only() {
    let dir = thumb_scratch("offline-earlier");
    let png = dir.join("exported.png");
    image::RgbImage::from_pixel(30, 20, image::Rgb([40, 190, 50]))
        .save(&png)
        .unwrap();
    let hash = greycard_library::hash_file(&png).unwrap();
    let stamp = file_stat(&png).unwrap().1;
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        let line = match o {
            Outcome::Thumbnail { index, width, .. } => format!("thumbnail {index} {width}"),
            Outcome::NoThumbnail { index, .. } => format!("none {index}"),
            _ => return,
        };
        let _ = tx.lock().unwrap().send(line);
    });
    let mut thumbs = Thumbs::at(dir.join("thumbs"), greycard_library::thumbs::DEFAULT_CAP);
    let made = crate::made::made_size(worker.pool().expect("a window's worker").size());
    let kept = Thumb {
        width: 5,
        height: 4,
        rgb: vec![90; 60],
    };
    let before = Tag {
        recipe: THUMB_RECIPE,
        stamp,
    };
    thumbs.put(&hash, made, before, &kept).unwrap();
    worker.set_thumb_cache(Some(thumbs));
    let job = |index| Job::CachedThumbnail {
        index,
        frame: Frame::file(png.clone()),
        hash: hash.clone(),
        stamp,
    };
    let wait = std::time::Duration::from_secs(60);
    worker.send(job(1));
    assert_eq!(rx.recv_timeout(wait).unwrap(), "none 1", "in reach");
    std::fs::remove_file(&png).unwrap();
    worker.send(job(2));
    assert_eq!(
        rx.recv_timeout(wait).unwrap(),
        "thumbnail 2 5",
        "out of reach"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A picture file is made once, kept, found; re-written in place,
/// it is made again, whatever its head says.
#[test]
fn a_picture_is_kept_until_it_is_written_again() {
    let dir = thumb_scratch("picture");
    let png = dir.join("export.png");
    let write = |shade: u8| {
        image::RgbImage::from_pixel(40, 20, image::Rgb([shade, shade, shade]))
            .save(&png)
            .unwrap();
    };
    write(200);
    let cache = cache_at(&dir);
    let (first, cached) =
        cached_thumbnail_noting(&cache, None, None, &Frame::file(&png), 16, thumbnail).unwrap();
    assert!(!cached);
    let (again, cached) =
        cached_thumbnail_noting(&cache, None, None, &Frame::file(&png), 16, thumbnail).unwrap();
    assert!(cached);
    assert_eq!((again.width, again.height), (first.width, first.height));
    // Written again with another mtime.
    write(20);
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(&png)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let (fresh, cached) =
        cached_thumbnail_noting(&cache, None, None, &Frame::file(&png), 16, thumbnail).unwrap();
    assert!(!cached);
    assert!(
        fresh.rgb.iter().all(|v| *v < 60),
        "the new picture's pixels"
    );
    crate::testing::remove_dir_retry(&dir);
}

/// A stand-in for the decode: a picture whose every byte is the
/// file's last, which is zero while a preallocated copy has not
/// reached its tail, as a cut preview decodes grey.
fn tail_picture(path: &std::path::Path, _size: u32) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let bytes = std::fs::read(path)?;
    let last = *bytes.last().unwrap_or(&0);
    Ok((4, 3, vec![last; 36]))
}

fn set_time(path: &std::path::Path, secs_from_now: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(secs_from_now))
        .unwrap();
}

/// The review's recipe: a raw whose copier set its length first
/// and has written only its head. Its key is already the finished
/// file's, so what keeps the half-made picture from standing for
/// the finished file is the time the copy moves on.
#[test]
fn a_raw_caught_half_copied_is_made_again_when_the_copy_ends() {
    let dir = thumb_scratch("halfcopied");
    let whole: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8 | 1).collect();
    let raw = dir.join("DSC_1981.NEF");
    // `truncate -s` to the full length, then the first 192 KB.
    let mut half = vec![0u8; whole.len()];
    half[..196_608].copy_from_slice(&whole[..196_608]);
    std::fs::write(&raw, &half).unwrap();
    let cache = cache_at(&dir);
    let (grey, cached) =
        cached_thumbnail_noting(&cache, None, None, &Frame::file(&raw), 176, tail_picture).unwrap();
    assert!(!cached);
    assert!(grey.rgb.iter().all(|v| *v == 0));
    // The same head and the same length: the same hash.
    let before = greycard_library::hash_file(&raw).unwrap();
    std::fs::write(&raw, &whole).unwrap();
    set_time(&raw, 7);
    assert_eq!(greycard_library::hash_file(&raw).unwrap(), before);
    let (done, cached) =
        cached_thumbnail_noting(&cache, None, None, &Frame::file(&raw), 176, tail_picture).unwrap();
    assert!(
        !cached,
        "the half-copied picture is not the finished file's"
    );
    assert!(done.rgb.iter().all(|v| *v == *whole.last().unwrap()));
    // And the finished file's picture is kept, and a rename keeps
    // its time and finds it.
    let renamed = dir.join("wedding-1981.NEF");
    std::fs::rename(&raw, &renamed).unwrap();
    let (again, cached) = cached_thumbnail_noting(
        &cache,
        None,
        None,
        &Frame::file(&renamed),
        176,
        tail_picture,
    )
    .unwrap();
    assert!(cached);
    assert_eq!(again.rgb, done.rgb);
    crate::testing::remove_dir_retry(&dir);
}

/// A card's frame and its import by a plain `cp`: one head, one
/// length, two times. Each is made once, and after that both hit,
/// opened by turns, rather than each removing the other's entry.
#[test]
fn two_copies_with_two_times_both_hit() {
    let dir = thumb_scratch("twocopies");
    let card = dir.join("card").join("IMG_0007.CR3");
    let import = dir.join("import").join("IMG_0007.CR3");
    std::fs::create_dir_all(card.parent().unwrap()).unwrap();
    std::fs::create_dir_all(import.parent().unwrap()).unwrap();
    std::fs::write(&card, vec![7u8; 90_000]).unwrap();
    std::fs::copy(&card, &import).unwrap();
    set_time(&import, 60);
    assert_eq!(
        greycard_library::hash_file(&card).unwrap(),
        greycard_library::hash_file(&import).unwrap()
    );
    let cache = cache_at(&dir);
    for (round, path) in [&card, &import, &card, &import, &card]
        .into_iter()
        .enumerate()
    {
        let (_, cached) =
            cached_thumbnail_noting(&cache, None, None, &Frame::file(path), 176, tail_picture)
                .unwrap();
        assert_eq!(cached, round >= 2, "round {round}");
    }
    crate::testing::remove_dir_retry(&dir);
}

/// A file that grows or is touched while its picture is being made
/// gets the picture shown and not kept.
#[test]
fn a_file_changing_under_the_making_is_not_kept() {
    let dir = thumb_scratch("changing");
    let raw = dir.join("IMG_0002.CR3");
    std::fs::write(&raw, vec![5u8; 80_000]).unwrap();
    let cache = cache_at(&dir);
    let grows = |path: &std::path::Path, size: u32| {
        let made = tail_picture(path, size);
        let mut f = std::fs::File::options().append(true).open(path).unwrap();
        std::io::Write::write_all(&mut f, &[9u8; 1000]).unwrap();
        made
    };
    let (_, cached) =
        cached_thumbnail_noting(&cache, None, None, &Frame::file(&raw), 176, grows).unwrap();
    assert!(!cached);
    let touched = |path: &std::path::Path, size: u32| {
        let made = tail_picture(path, size);
        set_time(path, 30);
        made
    };
    let (_, cached) =
        cached_thumbnail_noting(&cache, None, None, &Frame::file(&raw), 176, touched).unwrap();
    assert!(!cached);
    assert_eq!(
        cache.lock().unwrap().as_ref().unwrap().usage().entries,
        0,
        "neither was kept"
    );
    // Left alone, it is made once and kept.
    let (_, cached) =
        cached_thumbnail_noting(&cache, None, None, &Frame::file(&raw), 176, tail_picture).unwrap();
    assert!(!cached);
    let (_, cached) =
        cached_thumbnail_noting(&cache, None, None, &Frame::file(&raw), 176, tail_picture).unwrap();
    assert!(cached);
    crate::testing::remove_dir_retry(&dir);
}

/// A cache with a cap of nothing is off: nothing looked up, nothing
/// written, though it is still there to be cleared.
#[test]
fn a_cap_of_nothing_is_the_cache_off() {
    let dir = thumb_scratch("capzero");
    let png = dir.join("a.png");
    image::RgbImage::from_pixel(8, 8, image::Rgb([9, 9, 9]))
        .save(&png)
        .unwrap();
    let cache: ThumbCache = Arc::new(Mutex::new(Some(Thumbs::at(dir.join("thumbs"), 0))));
    for _ in 0..2 {
        let (_, cached) =
            cached_thumbnail_noting(&cache, None, None, &Frame::file(&png), 8, thumbnail).unwrap();
        assert!(!cached);
    }
    assert!(!dir.join("thumbs").exists());
    crate::testing::remove_dir_retry(&dir);
}

/// The count runs off the caller's thread and seeds the cache.
#[test]
fn the_cache_is_counted_on_a_thread_of_its_own() {
    let dir = thumb_scratch("count");
    let root = dir.join("thumbs");
    let mut filled = Thumbs::at(&root, greycard_library::thumbs::DEFAULT_CAP);
    let kept = Thumb {
        width: 3,
        height: 2,
        rgb: vec![128; 18],
    };
    filled
        .put(&"ab".repeat(32), 128, Tag::default(), &kept)
        .unwrap();
    let cache: ThumbCache = Arc::new(Mutex::new(Some(Thumbs::at(
        &root,
        greycard_library::thumbs::DEFAULT_CAP,
    ))));
    let (tx, rx) = std::sync::mpsc::channel();
    count_thumb_cache(&cache, move |known| tx.send(known).unwrap());
    let known = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap()
        .expect("counted");
    assert_eq!(known.entries, 1);
    assert_eq!(
        cache.lock().unwrap().as_ref().unwrap().known_usage(),
        Some(known)
    );
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn without_a_cache_a_thumbnail_is_made_every_time() {
    let dir = thumb_scratch("off");
    let png = dir.join("a.png");
    image::RgbImage::from_pixel(8, 8, image::Rgb([9, 9, 9]))
        .save(&png)
        .unwrap();
    let off: ThumbCache = Arc::new(Mutex::new(None));
    for _ in 0..2 {
        let (_, cached) =
            cached_thumbnail_noting(&off, None, None, &Frame::file(&png), 8, thumbnail).unwrap();
        assert!(!cached);
    }
    assert!(!dir.join("thumbs").exists());
    crate::testing::remove_dir_retry(&dir);
}

/// A learned mask the edit asks for whose model is not in the
/// store is left out of the picture, as the window draws it, and
/// said: the export does not pass over it in silence. An Object
/// with nothing picked asks for nothing and is not said, nor is a
/// shape switched off or one in an adjustment switched off, which
/// the finish never draws.
#[test]
fn an_export_says_which_learned_mask_it_went_without() {
    use greycard_edit::mask::Component;
    let frame = Arc::new(aberrated_frame(160, 120));
    let mut base = None;
    develop_once(&frame, &mut base, &mut None);
    let empty = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-scratch")
        .join(format!("left-out-{}/models", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let mut ai = Ai::for_test(greycard_ai::Store::at(&empty), Vec::new());
    let mut edit = Edit::default();
    edit.adjustments.push(greycard_edit::Adjustment {
        name: "Face".into(),
        mask: greycard_edit::mask::Mask {
            components: vec![
                Component {
                    shape: Shape::Subject {},
                    ..Component::default()
                },
                Component {
                    shape: Shape::Object {
                        picks: Vec::new(),
                        boxes: Vec::new(),
                    },
                    ..Component::default()
                },
            ],
            invert: false,
            ..Default::default()
        },
        ..greycard_edit::Adjustment::default()
    });
    // An adjustment switched off is not drawn, so its Subject is
    // not missed; nor is a Subject shape switched off.
    edit.adjustments.push(greycard_edit::Adjustment {
        name: "Off".into(),
        enabled: false,
        mask: greycard_edit::mask::Mask {
            components: vec![Component {
                shape: Shape::Subject {},
                ..Component::default()
            }],
            invert: false,
            ..Default::default()
        },
        ..greycard_edit::Adjustment::default()
    });
    edit.adjustments.push(greycard_edit::Adjustment {
        name: "Shape off".into(),
        mask: greycard_edit::mask::Mask {
            components: vec![Component {
                shape: Shape::Subject {},
                enabled: false,
                ..Component::default()
            }],
            invert: false,
            ..Default::default()
        },
        ..greycard_edit::Adjustment::default()
    });
    let (_, left_out) = export_held(
        &frame,
        &edit,
        base.as_ref(),
        &mut ai,
        &greycard_render::export::Settings::default(),
    );
    crate::testing::remove_dir_retry(empty.parent().unwrap());
    assert_eq!(left_out.len(), 1, "{left_out:?}");
    assert!(
        left_out[0].starts_with("Face's Subject shape:"),
        "{}",
        left_out[0]
    );
}

/// A develop whose learned denoiser or fill model is missing says
/// so for the export; one that asked for neither says nothing.
#[test]
fn a_develop_without_its_model_says_what_was_left_out() {
    let developed = |learned, fills| Outcome::Developed {
        generation: 0,
        turn: 0,
        kind: greycard_render::finish::Source::SCENE,
        image: Developed::Halves(Arc::new(Halves {
            width: 1,
            height: 1,
            pixels: vec![half::f16::ZERO; 4],
        })),
        guide: Arc::new(greycard_render::finish::Guide::NONE),
        planes: None,
        white: WhiteBase::IDENTITY,
        seconds: 0.0,
        detail: None,
        sharpen: None,
        dehaze: None,
        sources: Vec::new(),
        learned,
        fills,
        settles: false,
    };
    assert!(left_out_of(&developed(LearnedReport::Off, FillReport::default()), None).is_empty());
    let model = greycard_ai::denoiser(greycard_edit::Learned::Fast.tier().unwrap()).unwrap();
    let said = left_out_of(
        &developed(
            LearnedReport::Missing(model),
            FillReport {
                missing: vec![
                    ("Patch 1".into(), &greycard_ai::FILL),
                    ("Patch 2".into(), &greycard_ai::REMOVE),
                ],
                no_gpu: vec!["Patch 3".into()],
                ..FillReport::default()
            },
        ),
        None,
    );
    assert_eq!(said.len(), 4, "{said:?}");
    assert!(said[0].contains(model.name), "{}", said[0]);
    assert!(said[1].starts_with("fill Patch 1:"), "{}", said[1]);
    // A patch the generative remove made, on a machine without it,
    // names the model it waits on.
    assert!(said[2].starts_with("fill Patch 2:"), "{}", said[2]);
    assert!(said[2].contains(greycard_ai::REMOVE.name), "{}", said[2]);
    assert!(said[2].ends_with("left as it was"), "{}", said[2]);
    // One it made, on a machine without a GPU, says so instead.
    assert_eq!(
        said[3],
        "fill Patch 3: the remove needs a GPU; left as it was"
    );
}

/// A base made as the engine's stand-in for a learned denoiser
/// whose model is missing keeps saying so when a later develop
/// keeps it: that develop reports `Kept`, and the export of its
/// picture must still say what it went without.
#[test]
fn a_kept_stand_in_base_still_says_its_denoiser_was_missing() {
    let frame = Arc::new(aberrated_frame(160, 120));
    let empty = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-scratch")
        .join(format!("stand-in-{}/models", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let mut ai = Ai::for_test(greycard_ai::Store::at(&empty), Vec::new());
    let mut edit = Edit::default();
    edit.noise.enabled = true;
    edit.noise.learned = greycard_edit::Learned::Fast;
    let deliver: Deliver = Arc::new(|_| {});
    let input = Input::Raw(frame);
    let mut base = None;
    let mut develop = |edit: &Edit, base: &mut Option<FrameKept>| {
        develop_job(
            &input, edit, 0, 1, base, &mut ai, None, None, &mut None, &deliver,
        )
        .0
    };
    let first = develop(&edit, &mut base);
    // Without ONNX Runtime the denoiser is not missing but cannot run,
    // said with the runtime's line; the kept base says that instead.
    let runtime = greycard_ai::runtime::status().err();
    if let Some(why) = runtime {
        greycard_ai::runtime::absent_in_test(why);
        assert!(
            matches!(
                &first,
                Outcome::Developed {
                    learned: LearnedReport::Failed(m),
                    ..
                } if *m == why.to_string()
            ),
            "the denoiser should fail with the runtime's line"
        );
    } else {
        assert!(matches!(
            &first,
            Outcome::Developed {
                learned: LearnedReport::Missing(_),
                ..
            }
        ));
    }
    // A move that keeps the base: the exposure.
    edit.light.exposure = 0.5;
    let second = develop(&edit, &mut base);
    crate::testing::remove_dir_retry(empty.parent().unwrap());
    assert!(matches!(
        &second,
        Outcome::Developed {
            learned: LearnedReport::Kept,
            ..
        }
    ));
    assert!(left_out_of(&second, None).is_empty(), "the outcome alone");
    let said = left_out_of(&second, base.as_ref());
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].starts_with("the learned denoiser"), "{}", said[0]);
}

/// The viewport's half-float texture read back as a working image,
/// its alpha dropped.
pub(super) fn read_viewport(
    ctx: &greycard_gpu::Context,
    texture: &greycard_gpu::wgpu::Texture,
) -> WorkingImage {
    use greycard_gpu::wgpu;
    let (w, h) = (texture.width(), texture.height());
    let row = (w * 8).div_ceil(256) * 256;
    let band = ((64u32 << 20) / row).max(1);
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y0 in (0..h).step_by(band as usize) {
        let rows = band.min(h - y0);
        let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("read back"),
            size: u64::from(row * rows),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx.device().create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: y0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(rows),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue().submit(Some(encoder.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = slice.get_mapped_range().unwrap();
        for r in 0..rows as usize {
            let line: &[half::f16] =
                bytemuck::cast_slice(&mapped[r * row as usize..][..(w * 8) as usize]);
            for px in line.as_chunks::<4>().0 {
                data.extend([px[0].to_f32(), px[1].to_f32(), px[2].to_f32()]);
            }
        }
    }
    WorkingImage::from_data(w as usize, h as usize, data).unwrap()
}

/// A raw's develop names its body's baseline to everything finished
/// from it (the viewport, the export, the camera match): the GFX 100S
/// II's own, and the default for a body not in the table.
#[test]
fn a_raw_s_develop_carries_its_body_s_baseline() {
    let mut frame = aberrated_frame(120, 80);
    (frame.make, frame.model) = (crate::testing::GFX.0.into(), crate::testing::GFX.1.into());
    let mut base = None;
    develop_once(&Arc::new(frame.clone()), &mut base, &mut None);
    let b = greycard_edit::baseline::for_body(Some("Fujifilm GFX 100S II"));
    assert_eq!(
        base.as_ref().expect("a base").meta.source,
        greycard_render::finish::Source::Scene(greycard_render::finish::Baseline::of_stops(b))
    );
    (frame.make, frame.model) = ("Canon".into(), "Canon EOS R5 Mark II".into());
    let mut base = None;
    develop_once(&Arc::new(frame), &mut base, &mut None);
    assert_eq!(
        base.expect("a base").meta.source,
        greycard_render::finish::Source::SCENE
    );
}

/// The shelf's key for `frame`, as an open of a file at `path` makes it.
fn shelf_key(path: &str, frame: &Arc<RawFrame>) -> super::shelf::FileKey {
    super::shelf::FileKey {
        path: path.into(),
        stat: super::shelf::Stat::default(),
        frame: None,
    }
    .decoded(&Input::Raw(frame.clone()))
}

/// A frame left and opened again develops from the base it left on
/// the shelf: the base not made again, and said to be from the shelf
/// only on its first develop, and the base and the picture both the
/// ones a fresh develop of the same file under the same edit makes, to
/// the bit. Under an edit with another base it is made afresh.
#[test]
fn a_base_taken_back_from_the_shelf_is_developed_from_and_not_made_again() {
    use super::engine::swap_base;
    use super::shelf::{Budget, Shelf};
    let frame = Arc::new(aberrated_frame(120, 80));
    let edit = Edit::default();
    let key = shelf_key("a.CR3", &frame);
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    let mut base = None;
    let first = develop_turned(&frame, &edit, 0, &mut base);
    let made = base.as_ref().expect("a base").image.clone();

    // Away to another file, whose base is made, and back.
    swap_base(
        &mut base,
        &mut shelf,
        (Some(key.clone()), None),
        (&edit, 0, &|_, _| false),
        None,
        None,
    );
    assert!(base.is_none());
    develop_turned(&Arc::new(aberrated_frame(64, 48)), &edit, 0, &mut base);
    // The file read again, as an open reads it: another frame in
    // memory, which the shelf's key vouches for.
    let reread = Arc::new((*frame).clone());
    swap_base(
        &mut base,
        &mut shelf,
        (None, Some((&key, &Input::Raw(reread.clone())))),
        (&edit, 0, &|_, _| false),
        None,
        None,
    );
    assert!(base.as_ref().is_some_and(|b| b.from_shelf));
    let again = develop_turned(&reread, &edit, 0, &mut base);
    let b = base.as_ref().expect("the base");
    assert!(Arc::ptr_eq(&b.image, &made), "kept, not made again");
    assert!(!b.from_shelf, "said once");
    assert!(again.data == first.data);
    // A fresh develop of the same file under the same edit, from
    // nothing: the same base and the same picture.
    let mut fresh = None;
    let made_fresh = develop_turned(&frame, &edit, 0, &mut fresh);
    let fresh = fresh.expect("a fresh base");
    assert!(!Arc::ptr_eq(&fresh.image, &made));
    assert!(fresh.image.data == base.as_ref().unwrap().image.data);
    assert!(again.data == made_fresh.data);

    // Under another white balance the base is the edit's own.
    let warmer = Edit {
        white_balance: greycard_edit::WhiteBalance::Custom {
            temperature: 6500.0,
            tint: 0.0,
        },
        ..edit.clone()
    };
    swap_base(
        &mut base,
        &mut shelf,
        (Some(key.clone()), Some((&key, &Input::Raw(frame.clone())))),
        (&warmer, 0, &|_, _| false),
        None,
        None,
    );
    assert!(base.is_none());
    develop_turned(&frame, &warmer, 0, &mut base);
    assert!(!Arc::ptr_eq(&base.as_ref().unwrap().image, &made));
}

/// A camera profile in `dir` named `name`, its color matrix led by
/// `red`: another `red` is another profile's tables.
fn write_profile(dir: &std::path::Path, name: &str, red: f64) {
    use greycard_core::dcp::{Dcp, DcpCalibration, EmbedPolicy, HueSatMap};
    let dcp = Dcp {
        name: Some(name.into()),
        unique_camera_model: Some("Test Cam".into()),
        copyright: Some("public domain".into()),
        calibration_signature: None,
        embed_policy: EmbedPolicy::AllowCopying,
        primary: DcpCalibration {
            illuminant: 17,
            color_matrix: [red, -0.1, -0.1, -0.4, 1.3, 0.1, 0.0, -0.2, 0.9],
            forward_matrix: None,
            hue_sat_map: Some(HueSatMap::identity(6, 2, 1)),
        },
        secondary: None,
        baseline_exposure_offset: None,
        look: None,
        tone_curve: None,
    };
    std::fs::write(dir.join(format!("{name}.dcp")), dcp.to_bytes()).unwrap();
}

/// The profile directory `dir` the lists of this test's thread read.
fn profiles_in(dir: Option<&std::path::Path>) {
    crate::stages::TEST_STORES.with(|s| {
        *s.borrow_mut() = dir.map(|d| greycard_edit_io::Stores {
            profiles: Some(d.to_path_buf()),
            looks: None,
        })
    });
}

/// Two frames under an edit that names a camera profile, opened in
/// turn and returned to: each return takes its base back from the
/// shelf, though the profile cache let go of the profile between
/// (`profiles::forget` here, standing for a cache past its count or a
/// file rewritten), so the develop reads the same file again into
/// another pointer. The shelf
/// holds one base a file, never the one it was given back beside a
/// newer one. A profile rewritten with other tables is another base.
#[test]
fn a_base_under_a_camera_profile_is_taken_back_after_the_directory_is_listed() {
    use super::engine::swap_base;
    use super::shelf::{Budget, FileKey, Shelf};
    let dir = crate::testing::scratch_dir("shelf-profile");
    write_profile(&dir, "Test Cam Neutral", 0.8);
    profiles_in(Some(&dir));
    let mut edit = Edit::default();
    edit.camera.profile = greycard_edit::camera::ProfileChoice::Named("Test Cam Neutral".into());
    let a = Arc::new(aberrated_frame(120, 80));
    let b = Arc::new(aberrated_frame(64, 48));
    let (ka, kb) = (shelf_key("a.CR2", &a), shelf_key("b.CR2", &b));
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    let mut base = None;
    develop_turned(&a, &edit, 0, &mut base);
    let made_a = base.as_ref().expect("a base").image.clone();
    // An open of `to` from `from`, as the worker's: the base in hand
    // shelved and the coming one looked for, then the profile cache
    // let go of, then the develop. Whether the base was
    // taken from the shelf, and the picture.
    let open = |base: &mut Option<FrameKept>,
                shelf: &mut Shelf,
                from: &FileKey,
                to: (&FileKey, &Arc<RawFrame>)| {
        swap_base(
            base,
            shelf,
            (Some(from.clone()), Some((to.0, &Input::Raw(to.1.clone())))),
            (&edit, 0, &|_, _| false),
            None,
            None,
        );
        greycard_edit_io::profiles::forget();
        let taken = base.as_ref().is_some_and(|b| b.from_shelf);
        let picture = develop_turned(to.1, &edit, 0, base);
        (taken, picture)
    };
    assert!(!open(&mut base, &mut shelf, &ka, (&kb, &b)).0, "B is new");
    let made_b = base.as_ref().expect("a base").image.clone();
    let (taken, back_a) = open(&mut base, &mut shelf, &kb, (&ka, &a));
    assert!(taken, "A from the shelf");
    assert!(Arc::ptr_eq(&base.as_ref().unwrap().image, &made_a));
    assert!(
        open(&mut base, &mut shelf, &ka, (&kb, &b)).0,
        "B from the shelf"
    );
    assert!(Arc::ptr_eq(&base.as_ref().unwrap().image, &made_b));
    assert_eq!(shelf.len(), 1, "A's base alone on the shelf");

    // A's base made again from nothing, after another listing, and
    // shelved beside the one already there: it replaces it.
    let mut again = None;
    greycard_edit_io::profiles::forget();
    let fresh = develop_turned(&a, &edit, 0, &mut again);
    assert!(!Arc::ptr_eq(&again.as_ref().unwrap().image, &made_a));
    // The base taken back, and the picture developed from it, are the
    // ones a fresh develop under the profile makes, to the bit; and the
    // profile is in them, not the file's own.
    assert!(again.as_ref().unwrap().image.data == made_a.data);
    assert!(fresh.data == back_a.data);
    let own = develop_turned(&a, &Edit::default(), 0, &mut None);
    assert!(own.data != fresh.data, "the profile is applied");
    assert!(shelf.put(ka.clone(), again.take().unwrap(), false));
    assert_eq!(shelf.len(), 1, "one base for one file under one edit");

    // The profile rewritten with other tables: A's base is not its.
    write_profile(&dir, "Test Cam Neutral", 0.9);
    greycard_edit_io::profiles::forget();
    assert!(
        !open(&mut base, &mut shelf, &kb, (&ka, &a)).0,
        "another profile"
    );
    assert!(!Arc::ptr_eq(&base.as_ref().unwrap().image, &made_a));
    profiles_in(None);
    crate::testing::remove_dir_retry(&dir);
}

/// The open frame's base under a camera profile is kept across a
/// listing of the profile directory, which lets go of the profile read
/// before it: a slider move after the listing develops from the base in
/// hand, not from a base made again.
#[test]
fn the_base_in_hand_under_a_camera_profile_outlives_a_listing() {
    let dir = crate::testing::scratch_dir("hand-profile");
    write_profile(&dir, "Test Cam Neutral", 0.8);
    profiles_in(Some(&dir));
    let mut edit = Edit::default();
    edit.camera.profile = greycard_edit::camera::ProfileChoice::Named("Test Cam Neutral".into());
    let frame = Arc::new(aberrated_frame(120, 80));
    let mut base = None;
    develop_turned(&frame, &edit, 0, &mut base);
    let made = base.as_ref().expect("a base").image.clone();
    greycard_edit_io::profiles::forget();
    let mut moved = edit.clone();
    moved.light.exposure += 0.5;
    develop_turned(&frame, &moved, 0, &mut base);
    assert!(
        Arc::ptr_eq(&base.as_ref().unwrap().image, &made),
        "kept, not made again"
    );
    profiles_in(None);
    crate::testing::remove_dir_retry(&dir);
}

/// An export whose edit names a camera profile not in the profile
/// directory is developed under the file's own matrices and says so,
/// as a missing look is said, and so does one there that will not
/// read; with the profile there it says nothing and the profile is in
/// the picture.
#[test]
fn an_export_says_a_camera_profile_it_went_without() {
    let dir = crate::testing::scratch_dir("export-profile");
    write_profile(&dir, "Test Cam Neutral", 0.8);
    profiles_in(Some(&dir));
    let frame = Arc::new(aberrated_frame(120, 80));
    let settings = greycard_render::export::Settings::default();
    let mut ai = Ai::new();
    let mut edit = Edit::default();
    edit.camera.profile = greycard_edit::camera::ProfileChoice::Named("Test Cam Neutral".into());
    let pixels = |r: greycard_render::export::Rendered| -> Vec<u16> {
        match r.pixels {
            greycard_render::export::Pixels::Eight(p) => p.into_iter().map(u16::from).collect(),
            greycard_render::export::Pixels::Sixteen(p) => p,
        }
    };
    let (with, said) = export_held(&frame, &edit, None, &mut ai, &settings);
    assert!(said.is_empty(), "{said:?}");
    let (own, said) = export_held(&frame, &Edit::default(), None, &mut ai, &settings);
    assert!(said.is_empty(), "{said:?}");
    let (with, own) = (pixels(with), pixels(own));
    assert!(with != own, "the profile is applied");

    edit.camera.profile = greycard_edit::camera::ProfileChoice::Named("Camera/Gone".into());
    let (without, said) = export_held(&frame, &edit, None, &mut ai, &settings);
    assert_eq!(
        said,
        ["the camera profile Camera/Gone: not in the profile directory; written without it"]
    );
    assert!(
        pixels(without) == own,
        "the file's own, as the window draws it"
    );

    // A folder named like a profile is no profile: not there, and said.
    std::fs::create_dir_all(dir.join("Folder.dcp")).unwrap();
    edit.camera.profile = greycard_edit::camera::ProfileChoice::Named("Folder".into());
    let (folder, said) = export_held(&frame, &edit, None, &mut ai, &settings);
    assert_eq!(
        said,
        ["the camera profile Folder: not in the profile directory; written without it"]
    );
    assert!(pixels(folder) == own);

    // A profile there that will not read (cut short, or garbage):
    // developed under the file's own as the develop decides, and said.
    let whole = std::fs::read(dir.join("Test Cam Neutral.dcp")).unwrap();
    std::fs::write(dir.join("Broken.dcp"), &whole[..whole.len() / 3]).unwrap();
    std::fs::write(dir.join("Garbage.dcp"), b"not a profile").unwrap();
    for name in ["Broken", "Garbage"] {
        edit.camera.profile = greycard_edit::camera::ProfileChoice::Named(name.into());
        let (broken, said) = export_held(&frame, &edit, None, &mut ai, &settings);
        assert_eq!(
            said,
            [format!(
                "the camera profile {name}: it would not read; written without it"
            )]
        );
        assert!(
            pixels(broken) == own,
            "the file's own, as the develop has it"
        );
    }
    profiles_in(None);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame whose edit asks for the learned denoiser, with the model not
/// to be had: its base is the engine's own path standing in, shelved
/// when the frame is left and taken back on the return, the same
/// picture a fresh develop makes; the shelf holds one base of it.
#[test]
fn a_base_standing_in_for_the_learned_denoiser_is_taken_back() {
    use super::engine::{stands_in, swap_base};
    use super::shelf::{Budget, FileKey, Shelf};
    let mut edit = Edit::default();
    edit.noise.enabled = true;
    edit.noise.learned = greycard_edit::Learned::Fast;
    edit.noise.learned_strength = 0.5;
    // A store of its own with nothing in it: the machine's may hold
    // the model.
    let ai = || Ai::for_test(greycard_ai::Store::at("no-store"), Vec::new());
    let develop_turned =
        |frame: &Arc<RawFrame>, edit: &Edit, turn: u8, base: &mut Option<FrameKept>| {
            let deliver: Deliver = Arc::new(|_| {});
            let (outcome, image) = develop_job(
                &Input::Raw(frame.clone()),
                edit,
                turn,
                1,
                base,
                &mut ai(),
                None,
                None,
                &mut None,
                &deliver,
            );
            assert!(matches!(outcome, Outcome::Developed { .. }));
            image.expect("a picture on the CPU")
        };
    assert!(
        stands_in(&ai(), &edit, false),
        "no model in the test's store"
    );
    assert!(stands_in(&ai(), &edit, true), "nor its X-Trans network");
    let a = Arc::new(aberrated_frame(120, 80));
    let b = Arc::new(aberrated_frame(64, 48));
    let (ka, kb) = (shelf_key("a.CR2", &a), shelf_key("b.CR2", &b));
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    let mut base = None;
    let first = develop_turned(&a, &edit, 0, &mut base);
    let made = base.as_ref().expect("a base").image.clone();
    assert!(base.as_ref().unwrap().meta.stood_in.is_some());
    let open = |base: &mut Option<FrameKept>,
                shelf: &mut Shelf,
                from: &FileKey,
                to: (&FileKey, &Arc<RawFrame>)| {
        swap_base(
            base,
            shelf,
            (Some(from.clone()), Some((to.0, &Input::Raw(to.1.clone())))),
            (&edit, 0, &|e, x| stands_in(&ai(), e, x)),
            None,
            None,
        );
        let taken = base.as_ref().is_some_and(|b| b.from_shelf);
        let picture = develop_turned(to.1, &edit, 0, base);
        (taken, picture)
    };
    assert!(!open(&mut base, &mut shelf, &ka, (&kb, &b)).0, "B is new");
    let (taken, again) = open(&mut base, &mut shelf, &kb, (&ka, &a));
    assert!(taken, "A from the shelf");
    assert!(Arc::ptr_eq(&base.as_ref().unwrap().image, &made));
    assert!(again.data == first.data);
    assert!(
        open(&mut base, &mut shelf, &ka, (&kb, &b)).0,
        "B from the shelf"
    );
    assert_eq!(shelf.len(), 1, "A's base alone on the shelf");
}

/// On the GPU, a base shelved lets go of what it held on the device
/// (the picture before the sharpen, the patched upload, and the
/// dehaze's reduced copy), and its first develop back uploads the
/// picture again and sharpens it there: the viewport's texture is the
/// one a fresh develop of the same file under the same edit leaves, to
/// the bit, and so is the base.
#[test]
fn a_base_taken_back_on_the_gpu_is_uploaded_again_and_develops_as_a_fresh_one() {
    use super::engine::swap_base;
    use super::shelf::{Budget, Shelf};
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the shelf's GPU take-back");
        return;
    };
    let mut gpu = Some(ctx);
    let frame = Arc::new(aberrated_frame(400, 320));
    let key = shelf_key("a.CR3", &frame);
    let edit = Edit::default();
    assert!(edit.sharpen.options().is_some(), "the sharpen on the GPU");
    let texture = |outcome: Outcome| match outcome {
        Outcome::Developed {
            image: Developed::Texture(t),
            ..
        } => t,
        _ => panic!("a texture on the device"),
    };
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    let mut base = None;
    let first = texture(develop_once(&frame, &mut base, &mut gpu).0);
    let b = base.as_ref().expect("a base");
    assert!(b.on_gpu && b.kept.has_upload());
    let made = b.image.clone();
    drop(first);

    swap_base(
        &mut base,
        &mut shelf,
        (Some(key.clone()), None),
        (&edit, 0, &|_, _| false),
        gpu.as_ref(),
        None,
    );
    develop_once(&Arc::new(aberrated_frame(200, 160)), &mut base, &mut gpu);
    swap_base(
        &mut base,
        &mut shelf,
        (None, Some((&key, &Input::Raw(frame.clone())))),
        (&edit, 0, &|_, _| false),
        gpu.as_ref(),
        None,
    );
    let b = base.as_ref().expect("taken back");
    assert!(Arc::ptr_eq(&b.image, &made) && b.from_shelf);
    assert!(!b.kept.on_device(Keep::PreSharpen) && !b.kept.has_upload() && !b.kept.has_haze());

    let back = texture(develop_once(&frame, &mut base, &mut gpu).0);
    let b = base.as_ref().expect("the base");
    assert!(Arc::ptr_eq(&b.image, &made), "kept, not made again");
    assert!(b.kept.has_upload(), "uploaded again");
    let ctx = gpu.as_ref().expect("the device kept");
    let back = read_viewport(ctx, &back);

    let mut fresh = None;
    let fresh_texture = texture(develop_once(&frame, &mut fresh, &mut gpu).0);
    let ctx = gpu.as_ref().expect("the device kept");
    let fresh_picture = read_viewport(ctx, &fresh_texture);
    assert!(fresh.expect("a fresh base").image.data == made.data);
    assert!(back.data == fresh_picture.data);
}

/// A scratch folder with linear DNGs of our own named `names`, each
/// `width` by `height`: raws the worker decodes from the disk.
fn dngs(what: &str, names: &[&str], width: usize, height: usize) -> (PathBuf, Vec<PathBuf>) {
    let dir = crate::testing::scratch_dir(what);
    let paths = names
        .iter()
        .map(|n| {
            let path = dir.join(n);
            crate::testing::linear_dng(&path, width, height, "Test", "Cam");
            super::prefetch::watch(n);
            path
        })
        .collect();
    (dir, paths)
}

fn neighbor_at(path: &std::path::Path, edit: Edit, side: Side) -> Neighbor {
    Neighbor {
        path: path.to_path_buf(),
        edit,
        turn: 0,
        seed_blend: false,
        side,
        steps: 1,
        read_first: None,
    }
}

/// A neighbor prefetched onto the shelf is the base its open takes
/// back, said to be from the shelf, and develops to the picture a
/// fresh open of the same file makes, to the bit. Asked again while it
/// is there, it is not made twice.
#[test]
fn a_neighbor_prefetched_is_taken_back_by_its_open() {
    use super::engine::swap_base;
    use super::prefetch::{Ahead, Fetched, prefetch, said};
    use super::shelf::{Budget, FileKey, Shelf};
    let (dir, paths) = dngs("prefetch-took", &["pf-took-B.dng"], 96, 64);
    let b = &paths[0];
    let edit = Edit::default();
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    let n = neighbor_at(b, edit.clone(), Side::Next);
    assert_eq!(
        prefetch(&n, None, &[], &mut shelf, &mut None, None, &|_| {
            Ahead::Nothing
        }),
        Fetched::Landed
    );
    assert_eq!(shelf.len(), 1);
    let lines = said("pf-took-B.dng");
    assert!(
        lines
            .iter()
            .any(|l| l == "prefetching the next frame, pf-took-B.dng"),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(
            |l| l.starts_with("prefetched pf-took-B.dng, the next frame: decode ")
                && l.contains(", prepare ")
                && l.contains(", demosaic ")
                && l.contains(", move into the working space ")
                && l.contains("; the shelf: 1 base(s)")
        ),
        "{lines:?}"
    );
    assert!(matches!(
        prefetch(&n, None, &[], &mut shelf, &mut None, None, &|_| Ahead::Nothing),
        Fetched::Skipped(why) if why == "its base is on the shelf"
    ));
    assert_eq!(shelf.len(), 1);

    // The open of it, as the worker's: the key from the file and its
    // decode, the base taken back, and the develop from it.
    let (input, _) = open(b).expect("our DNG decodes");
    let key = FileKey::of(b).expect("on the disk").decoded(&input);
    let mut base = None;
    swap_base(
        &mut base,
        &mut shelf,
        (None, Some((&key, &input))),
        (&edit, 0, &|_, _| false),
        None,
        None,
    );
    assert!(base.as_ref().is_some_and(|b| b.from_shelf), "taken back");
    assert!(
        said("pf-took-B.dng")
            .iter()
            .any(|l| l == "the base of pf-took-B.dng from the shelf")
    );
    let taken = base.as_ref().unwrap().image.clone();
    let deliver: Deliver = Arc::new(|_| {});
    let develop = |base: &mut Option<FrameKept>| {
        develop_job(
            &input,
            &edit,
            0,
            1,
            base,
            &mut Ai::new(),
            None,
            None,
            &mut None,
            &deliver,
        )
        .1
        .expect("a picture on the CPU")
    };
    let back = develop(&mut base);
    assert!(
        Arc::ptr_eq(&base.as_ref().unwrap().image, &taken),
        "not made again"
    );
    let mut fresh = None;
    let made = develop(&mut fresh);
    assert!(fresh.unwrap().image.data == taken.data);
    assert!(made.data == back.data);
    crate::testing::remove_dir_retry(&dir);
}

/// Anything asked for while a prefetch is in hand stops it at the next
/// stage, and nothing of it is kept: before the prepare (after the
/// decode), the demosaic, and the move into the working space. The open of
/// the very frame it is making does not; that is said once, and the
/// base lands.
#[test]
fn a_request_stops_a_prefetch_between_its_stages() {
    use super::prefetch::{Ahead, Fetched, prefetch, said};
    use super::shelf::{Budget, Shelf};
    let (dir, paths) = dngs("prefetch-stop", &["pf-stop-B.dng"], 96, 64);
    let n = neighbor_at(&paths[0], Edit::default(), Side::Previous);
    for (asked, stage) in [
        (1, "prepare"),
        (2, "demosaic"),
        (3, "move into the working space"),
    ] {
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        let calls = std::cell::Cell::new(0);
        let ahead = |_: &crate::stages::Facts| {
            calls.set(calls.get() + 1);
            if calls.get() >= asked {
                Ahead::Work
            } else {
                Ahead::Nothing
            }
        };
        assert!(matches!(
            prefetch(&n, None, &[], &mut shelf, &mut None, None, &ahead),
            Fetched::Abandoned(at, _) if at == stage
        ));
        assert_eq!(shelf.len(), 0, "nothing kept when stopped before {stage}");
        assert_eq!(calls.get(), asked, "asked once a stage");
    }
    assert!(said("pf-stop-B.dng").iter().any(|l| l
        == "the prefetch of the previous frame, pf-stop-B.dng, abandoned before its demosaic: \
            the worker was asked for something"));

    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    assert_eq!(
        prefetch(&n, None, &[], &mut shelf, &mut None, None, &|_| {
            Ahead::ItsOpen
        }),
        Fetched::Landed
    );
    assert_eq!(shelf.len(), 1);
    let goes_on = said("pf-stop-B.dng")
        .into_iter()
        .filter(|l| l.contains("goes on"))
        .collect::<Vec<_>>();
    assert_eq!(
        goes_on,
        vec![
            "the prefetch of pf-stop-B.dng goes on before its prepare: \
             it is the frame being opened"
        ]
    );
    crate::testing::remove_dir_retry(&dir);
}

/// A frame whose edit asks for the learned denoiser is skipped before
/// anything is read: its model would run for seconds with no stage to
/// stop between, and its pair would hold a gigabyte on the shelf. So
/// is the frame whose base is the one in hand, another version of the
/// open file under the same base.
#[test]
fn a_learned_frame_and_the_open_one_are_not_prefetched() {
    use super::prefetch::{Ahead, Fetched, prefetch, said, watch};
    use super::shelf::{Budget, Shelf};
    watch("pf-learned-B.CR3");
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    let learned = Edit {
        noise: Noise {
            learned: greycard_edit::Learned::Fast,
            ..Noise::default()
        },
        ..Edit::default()
    };
    // No such file: a decode would say so.
    let nowhere = PathBuf::from("/nowhere/pf-learned-B.CR3");
    assert_eq!(
        prefetch(
            &neighbor_at(&nowhere, learned, Side::Next),
            None,
            &[],
            &mut shelf,
            &mut None,
            None,
            &|_| Ahead::Nothing
        ),
        Fetched::Skipped("its edit asks for the learned denoiser".into())
    );
    assert_eq!(
        said("pf-learned-B.CR3"),
        vec![
            "the prefetch of the next frame, pf-learned-B.CR3, skipped: \
             its edit asks for the learned denoiser"
        ]
    );

    let (dir, paths) = dngs("prefetch-open", &["pf-open-A.dng"], 64, 48);
    let mut base = None;
    let frame = match open(&paths[0]).expect("our DNG decodes").0 {
        Input::Raw(f) => f,
        Input::Picture { .. } => panic!("a raw"),
    };
    develop_turned(&frame, &Edit::default(), 0, &mut base);
    let open_one = base.as_ref().expect("a base");
    assert!(matches!(
        prefetch(
            &neighbor_at(&paths[0], Edit::default(), Side::Next),
            Some((paths[0].as_path(), open_one)),
            &[],
            &mut shelf,
            &mut None,
            None,
            &|_| Ahead::Nothing
        ),
        Fetched::Skipped(why) if why == "its base is the open frame's"
    ));
    assert_eq!(shelf.len(), 0);
    crate::testing::remove_dir_retry(&dir);
}

/// The worker on its thread: an open, and once its develop has landed
/// and nothing else is asked for, the next frame's base and then the
/// previous one's made onto the shelf; the open of the next takes its
/// base from there.
#[test]
fn the_worker_prefetches_the_neighbors_once_idle_and_their_open_takes_the_base() {
    use super::prefetch::said;
    let (dir, paths) = dngs(
        "prefetch-run",
        &["pf-run-A.dng", "pf-run-B.dng", "pf-run-C.dng"],
        96,
        64,
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed { generation, .. } | Outcome::Failed { generation, .. } = o {
            tx.lock().unwrap().send(generation).unwrap();
        }
    });
    let open = |path: &PathBuf, generation: u64| Job::Open {
        path: path.clone(),
        version: None,
        edit: Edit::default(),
        generation,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    };
    let wait = std::time::Duration::from_secs(60);
    worker.send(open(&paths[0], 1));
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![
            neighbor_at(&paths[1], Edit::default(), Side::Next),
            neighbor_at(&paths[2], Edit::default(), Side::Previous),
        ],
    );
    assert_eq!(rx.recv_timeout(wait).expect("the open lands"), 1);
    let asked = Instant::now();
    let landed = |name: &str| {
        said(name)
            .iter()
            .any(|l| l.starts_with(&format!("prefetched {name}")))
    };
    while !(landed("pf-run-B.dng") && landed("pf-run-C.dng")) {
        assert!(asked.elapsed() < wait, "both prefetched");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let order: Vec<String> = said("pf-run-")
        .into_iter()
        .filter(|l| l.starts_with("prefetching"))
        .collect();
    assert_eq!(
        order,
        vec![
            "prefetching the next frame, pf-run-B.dng",
            "prefetching the previous frame, pf-run-C.dng"
        ]
    );
    assert!(said("pf-run-A.dng").is_empty(), "the open one is not");
    worker.send(open(&paths[1], 2));
    assert_eq!(rx.recv_timeout(wait).expect("the next lands"), 2);
    assert!(
        said("pf-run-B.dng")
            .iter()
            .any(|l| l == "the base of pf-run-B.dng from the shelf")
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A neighbor `steps` away on `side`, under the default edit.
fn deep_at(path: &std::path::Path, side: Side, steps: u8) -> Neighbor {
    Neighbor {
        steps,
        ..neighbor_at(path, Edit::default(), side)
    }
}

/// A worker that delivers each develop's generation to the channel it
/// gives back, and an open of `path` under the default edit.
fn walking_worker() -> (Worker, std::sync::mpsc::Receiver<u64>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed { generation, .. } | Outcome::Failed { generation, .. } = o {
            tx.lock().unwrap().send(generation).unwrap();
        }
    });
    (worker, rx)
}

fn open_default(path: &std::path::Path, generation: u64) -> Job {
    Job::Open {
        path: path.to_path_buf(),
        version: None,
        edit: Edit::default(),
        generation,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    }
}

/// Wait, to a minute, until `seen`.
fn until(what: &str, seen: &dyn Fn() -> bool) {
    let asked = Instant::now();
    while !seen() {
        assert!(
            asked.elapsed() < std::time::Duration::from_secs(60),
            "{what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Walking right, the window sends three ahead and one behind, and the
/// worker makes them nearest ahead first, then the one behind, then the
/// farther ahead. A step finds its base on the shelf, and so does the
/// step back after it, a reversal: the frame just left.
#[test]
fn a_walk_is_prefetched_deep_ahead_and_a_reversal_finds_its_base() {
    use super::prefetch::said;
    let names = [
        "pf-deep-0.dng",
        "pf-deep-1.dng",
        "pf-deep-2.dng",
        "pf-deep-3.dng",
        "pf-deep-4.dng",
        "pf-deep-5.dng",
    ];
    let (dir, p) = dngs("prefetch-deep", &names, 96, 64);
    let (worker, rx) = walking_worker();
    let wait = std::time::Duration::from_secs(60);
    let landed = |name: &str| {
        said(name)
            .iter()
            .any(|l| l.starts_with(&format!("prefetched {name}")))
    };
    let from_shelf = |name: &str| {
        said(name)
            .iter()
            .any(|l| *l == format!("the base of {name} from the shelf"))
    };

    worker.send(open_default(&p[1], 1));
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![
            deep_at(&p[2], Side::Next, 1),
            deep_at(&p[0], Side::Previous, 1),
            deep_at(&p[3], Side::Next, 2),
            deep_at(&p[4], Side::Next, 3),
        ],
    );
    assert_eq!(rx.recv_timeout(wait).expect("the open lands"), 1);
    until("all four made", &|| {
        [2, 0, 3, 4].iter().all(|&i| landed(names[i]))
    });
    let order: Vec<String> = said("pf-deep-")
        .into_iter()
        .filter(|l| l.starts_with("prefetching"))
        .collect();
    assert_eq!(
        order,
        vec![
            "prefetching the next frame, pf-deep-2.dng",
            "prefetching the previous frame, pf-deep-0.dng",
            "prefetching the second next frame, pf-deep-3.dng",
            "prefetching the third next frame, pf-deep-4.dng",
        ]
    );

    // A step right: from the shelf; its new far neighbor made, the ones
    // made already not made again.
    worker.send(open_default(&p[2], 2));
    worker.neighbors(
        2,
        Heading::steady(Side::Next),
        vec![
            deep_at(&p[3], Side::Next, 1),
            deep_at(&p[1], Side::Previous, 1),
            deep_at(&p[4], Side::Next, 2),
            deep_at(&p[5], Side::Next, 3),
        ],
    );
    assert_eq!(rx.recv_timeout(wait).expect("the step lands"), 2);
    assert!(from_shelf(names[2]));
    until("the new far one made", &|| landed(names[5]));
    for i in [3, 4] {
        assert_eq!(
            said(names[i])
                .iter()
                .filter(|l| l.starts_with("prefetched"))
                .count(),
            1,
            "{} made once",
            names[i]
        );
    }

    // A step back, still walking right after one step the other way:
    // the frame just left is on the shelf.
    worker.send(open_default(&p[1], 3));
    assert_eq!(rx.recv_timeout(wait).expect("the reversal lands"), 3);
    assert!(from_shelf(names[1]), "the reversal from the shelf");
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// The bytes the shelf counts for the base of the raw at `path`, made
/// as a prefetch makes it; and what the prefetch reckoned from its
/// decode before making it, which is never less.
fn shelved_bytes(path: &std::path::Path) -> u64 {
    use super::prefetch::{Ahead, Fetched, base_estimate, prefetch};
    use super::shelf::{Budget, Shelf};
    let mut shelf = Shelf::new(Budget::Bytes(u64::MAX));
    let n = deep_at(path, Side::Next, 1);
    assert_eq!(
        prefetch(&n, None, &[], &mut shelf, &mut None, None, &|_| {
            Ahead::Nothing
        }),
        Fetched::Landed
    );
    let facts = super::decode::open(path)
        .expect("our DNG decodes")
        .0
        .walked()
        .facts(0);
    let reckoned = base_estimate(&facts);
    assert!(
        reckoned >= shelf.bytes(),
        "{reckoned} against {}",
        shelf.bytes()
    );
    assert!(
        reckoned <= shelf.bytes() * 2,
        "{reckoned} against {}",
        shelf.bytes()
    );
    shelf.bytes()
}

/// Neighbors larger than the open frame, on a shelf counted in bases
/// of its size: the first fits beside the frame just left; the second
/// would push the frame just left off, and is not made, nor anything
/// the list names pushed off by it.
#[test]
fn a_neighbor_larger_than_the_open_frame_pushes_out_nothing_kept() {
    use super::prefetch::{Ahead, Fetched, prefetch, said};
    use super::shelf::{Budget, Shelf};
    let (dir, small) = dngs("prefetch-big-small", &["pf-big-left.dng"], 64, 48);
    let (dir2, big) = dngs("prefetch-big", &["pf-big-n1.dng", "pf-big-n2.dng"], 96, 64);
    let one = shelved_bytes(&small[0]);
    let two = shelved_bytes(&big[0]);
    assert!(two > one && two < 3 * one, "{two} against {one}");
    // Room for three of the open frame's size: two ahead, one behind.
    let mut shelf = Shelf::new(Budget::Bytes(3 * one));
    let none = |_: &crate::stages::Facts| Ahead::Nothing;
    let left = deep_at(&small[0], Side::Previous, 1);
    assert_eq!(
        prefetch(&left, None, &[], &mut shelf, &mut None, None, &none),
        Fetched::Landed
    );
    let keep = vec![big[0].clone(), small[0].clone(), big[1].clone()];
    let n1 = deep_at(&big[0], Side::Next, 1);
    assert_eq!(
        prefetch(&n1, None, &keep, &mut shelf, &mut None, None, &none),
        Fetched::Landed
    );
    let n2 = deep_at(&big[1], Side::Next, 2);
    assert!(matches!(
        prefetch(&n2, None, &keep, &mut shelf, &mut None, None, &none),
        Fetched::Skipped(why) if why == "its base would push the base of pf-big-left.dng off the shelf"
    ));
    assert!(
        !said("pf-big-n2.dng")
            .iter()
            .any(|l| l.starts_with("prefetched")),
        "not made"
    );
    assert_eq!(shelf.bytes(), one + two, "both kept");
    crate::testing::remove_dir_retry(&dir);
    crate::testing::remove_dir_retry(&dir2);
}

/// With room for one base, a compare of two frames keeps both: walking
/// right the next is made in place of the frame just left, but a step
/// back and the step forward again make nothing, so each return finds
/// its base on the shelf.
#[test]
fn a_compare_of_two_frames_with_room_for_one_stays_warm() {
    use super::prefetch::{QUIET, said};
    use super::shelf::Budget;
    let names = [
        "pf-one-0.dng",
        "pf-one-1.dng",
        "pf-one-2.dng",
        "pf-one-3.dng",
    ];
    let (dir, p) = dngs("prefetch-one", &names, 96, 64);
    let (sized, size) = dngs("prefetch-one-size", &["pf-one-size.dng"], 96, 64);
    let one = shelved_bytes(&size[0]);
    let (worker, rx) = walking_worker();
    worker.shelf_by_test(Budget::Bytes(one * 3 / 2));
    let wait = std::time::Duration::from_secs(60);
    let heading = |streak| Heading {
        ahead: Side::Next,
        streak,
    };
    let from_shelf = |name: &str| {
        said(name)
            .iter()
            .filter(|l| **l == format!("the base of {name} from the shelf"))
            .count()
    };
    let step = |at: usize, generation: u64, streak: u8| {
        worker.send(open_default(&p[at], generation));
        let near = [(at + 1, Side::Next), (at.wrapping_sub(1), Side::Previous)];
        worker.neighbors(
            generation,
            heading(streak),
            near.iter()
                .filter_map(|&(i, side)| p.get(i).map(|q| deep_at(q, side, 1)))
                .collect(),
        );
        assert_eq!(rx.recv_timeout(wait).expect("a develop"), generation);
    };
    // A walk under way: at 1, then 2, the next made each time.
    step(1, 1, 2);
    until("2 made", &|| {
        said(names[2]).iter().any(|l| l.starts_with("prefetched"))
    });
    step(2, 2, 3);
    assert_eq!(from_shelf(names[2]), 1);
    until("3 made", &|| {
        said(names[3]).iter().any(|l| l.starts_with("prefetched"))
    });
    // Back to 1, which the walk let go; then 2, 1, 2: all warm.
    step(1, 3, 0);
    std::thread::sleep(QUIET * 10);
    step(2, 4, 1);
    assert_eq!(from_shelf(names[2]), 2, "the return to 2");
    std::thread::sleep(QUIET * 10);
    step(1, 5, 0);
    assert_eq!(from_shelf(names[1]), 1, "the return to 1");
    std::thread::sleep(QUIET * 10);
    step(2, 6, 1);
    assert_eq!(from_shelf(names[2]), 3);
    assert_eq!(
        said(names[3])
            .iter()
            .filter(|l| l.starts_with("prefetched"))
            .count(),
        1,
        "nothing made during the compare"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
    crate::testing::remove_dir_retry(&sized);
}

/// On a shelf of three bases, two ahead and one behind, full: the
/// step back's own list is made, the bases it names and skips as
/// shelved brought forward first, so the one behind it makes pushes out
/// neither of them; the step on from there finds its base.
#[test]
fn a_reversal_on_a_full_shelf_makes_its_own_list() {
    use super::prefetch::said;
    use super::shelf::Budget;
    let names = [
        "pf-full-0.dng",
        "pf-full-1.dng",
        "pf-full-2.dng",
        "pf-full-3.dng",
        "pf-full-4.dng",
        "pf-full-5.dng",
    ];
    let (dir, p) = dngs("prefetch-full", &names, 96, 64);
    let (sized, size) = dngs("prefetch-full-size", &["pf-full-size.dng"], 96, 64);
    let one = shelved_bytes(&size[0]);
    let (worker, rx) = walking_worker();
    // Room for three prefetched bases and a half; the frames left
    // carry their guide plane as well (a third again), so with one of
    // them on it the shelf is full.
    worker.shelf_by_test(Budget::Bytes(one * 7 / 2));
    let wait = std::time::Duration::from_secs(60);
    let made = |i: usize| {
        said(names[i])
            .iter()
            .filter(|l| l.starts_with("prefetched"))
            .count()
    };
    let from_shelf = |i: usize| {
        said(names[i])
            .iter()
            .any(|l| *l == format!("the base of {} from the shelf", names[i]))
    };
    let step = |at: usize, generation: u64, streak: u8| {
        worker.send(open_default(&p[at], generation));
        let near = [
            (at + 1, Side::Next, 1),
            (at.wrapping_sub(1), Side::Previous, 1),
            (at + 2, Side::Next, 2),
            (at + 3, Side::Next, 3),
        ];
        worker.neighbors(
            generation,
            Heading {
                ahead: Side::Next,
                streak,
            },
            near.iter()
                .filter_map(|&(i, side, steps)| p.get(i).map(|q| deep_at(q, side, steps)))
                .collect(),
        );
        assert_eq!(rx.recv_timeout(wait).expect("a develop"), generation);
    };
    let skipped_on_shelf = |i: usize| {
        said(names[i])
            .iter()
            .filter(|l| l.ends_with("skipped: its base is on the shelf"))
            .count()
    };
    step(2, 1, 2);
    until("3, 1, 4 made", &|| {
        made(3) == 1 && made(1) == 1 && made(4) == 1
    });
    assert_eq!(made(5), 0, "room for two ahead");
    step(3, 2, 3);
    assert!(from_shelf(3));
    until("5 made", &|| made(5) == 1);
    // The shelf: 5, 2 (just left), 4, with no room for another. Back to
    // 2, still walking right: 3 and 4 shelved, 1 to make.
    step(2, 3, 0);
    assert!(from_shelf(2));
    until("1 made again", &|| made(1) == 2);
    until("4 skipped as shelved", &|| skipped_on_shelf(4) >= 2);
    assert_eq!(made(4), 1, "4 kept, not made again");
    assert_eq!(made(3), 1, "3 kept, not made again");
    // The list's far one kept through the reversal, and 1, which it
    // made, still there after.
    worker.send(open_default(&p[4], 4));
    assert_eq!(rx.recv_timeout(wait).expect("a develop"), 4);
    assert!(
        from_shelf(4),
        "the list's far one kept through the reversal"
    );
    worker.send(open_default(&p[1], 5));
    assert_eq!(rx.recv_timeout(wait).expect("a develop"), 5);
    assert!(from_shelf(1));
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
    crate::testing::remove_dir_retry(&sized);
}

/// A new open's list replaces the last: a far neighbor of the last
/// still waiting is never made, one in hand gives way, and a frame that
/// is a neighbor of both is made under the new list's place.
#[test]
fn a_new_open_drops_the_far_prefetches() {
    use super::prefetch::{AtCheck, at_next_check, said};
    let names = [
        "pf-drop-0.dng",
        "pf-drop-1.dng",
        "pf-drop-2.dng",
        "pf-drop-3.dng",
        "pf-drop-4.dng",
        "pf-drop-5.dng",
        "pf-drop-6.dng",
    ];
    let (dir, p) = dngs("prefetch-drop", &names, 96, 64);
    let (worker, rx) = walking_worker();
    let wait = std::time::Duration::from_secs(60);
    let said_one = |name: &str, line: &str| said(name).iter().any(|l| l.contains(line));
    let (at_tx, at_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    at_next_check(&p[3], AtCheck::Hold(at_tx, go_rx));

    worker.send(open_default(&p[1], 1));
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![
            deep_at(&p[2], Side::Next, 1),
            deep_at(&p[0], Side::Previous, 1),
            deep_at(&p[3], Side::Next, 2),
            deep_at(&p[4], Side::Next, 3),
        ],
    );
    assert_eq!(rx.recv_timeout(wait).expect("the open lands"), 1);
    // The second next held in its prefetch; a jump to 5 meanwhile,
    // whose behind is 4, the third next of the last list.
    assert_eq!(at_rx.recv_timeout(wait).expect("held"), "prepare");
    worker.send(open_default(&p[5], 2));
    worker.neighbors(
        2,
        Heading::steady(Side::Next),
        vec![
            deep_at(&p[6], Side::Next, 1),
            deep_at(&p[4], Side::Previous, 1),
        ],
    );
    go_tx.send(()).unwrap();
    assert_eq!(rx.recv_timeout(wait).expect("the jump lands"), 2);
    until("4 made as 5's previous", &|| {
        said_one(names[4], "prefetched pf-drop-4.dng, the previous frame")
    });
    assert!(said_one(
        names[3],
        "the prefetch of the second next frame, pf-drop-3.dng, abandoned before its prepare"
    ));
    assert!(!said_one(names[3], "prefetched"), "3 never made");
    assert!(!said_one(names[4], "third next"), "the last list let go");
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A measurement, not a check: a walk right through the raws in
/// `GREYCARD_WALK_DIR`, the pauses after each develop delivered taken
/// in turn from `GREYCARD_WALK_GAPS_MS` (milliseconds, by commas; 2500
/// by default), so a long look and a burst of quick keys can be had,
/// timing each open from its send to its develop delivered,
/// which is the key to the picture less the window's own frame. The
/// neighbors go as the window sends them walking right; the shelf's
/// budget decides the depth, so a run with `GREYCARD_SHELF_MB` at room
/// for two bases of the frames walked, and not three, is the depth
/// before (one ahead, one behind): 600 for 24 MP frames, whose base the
/// shelf counts at 275 MB, but 1100 at 45 MP. On the
/// GPU when there is one: lavapipe under the recipe proves the walk
/// runs there, but its times are not a real card's.
///
/// WRITES SIDECARS. With `GREYCARD_WALK_EDITED` set (`before` or
/// `after`), every raw in `GREYCARD_WALK_DIR` without a sidecar gets one
/// written beside it, under a 6500 K white: point it only at a copy of
/// the raws. It refuses a folder under ~/Pictures or /mnt.
#[test]
#[ignore = "a measurement over real raws"]
fn walk_timing() {
    let Some(dir) = std::env::var_os("GREYCARD_WALK_DIR") else {
        eprintln!("GREYCARD_WALK_DIR is not set");
        return;
    };
    if std::env::var_os("GREYCARD_WALK_EDITED").is_some() {
        let at = std::fs::canonicalize(&dir).expect("the walk's folder");
        let pictures = std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Pictures"))
            .and_then(|p| std::fs::canonicalize(p).ok());
        assert!(
            !at.starts_with("/mnt") && !pictures.is_some_and(|p| at.starts_with(p)),
            "GREYCARD_WALK_EDITED writes sidecars: not in {}; walk a copy",
            at.display()
        );
    }
    let gaps: Vec<u64> = std::env::var("GREYCARD_WALK_GAPS_MS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    let gaps = if gaps.is_empty() { vec![2500] } else { gaps };
    let mut p: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the walk's folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| greycard_core::decode::is_raw_path(p) && p.is_file())
        .collect();
    p.sort();
    for path in &p {
        super::prefetch::watch(&super::engine::file_label(path));
    }
    let (worker, rx) = walking_worker();
    let device = {
        use greycard_gpu::wgpu;
        let instance = greycard_gpu::instance();
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()
            .and_then(|adapter| {
                eprintln!("adapter: {}", adapter.get_info().name);
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("walk timing"),
                    required_limits: adapter.limits(),
                    ..Default::default()
                }))
                .ok()
            })
    };
    match &device {
        Some((d, q)) => worker.set_gpu(d, q),
        None => skipped("the walk's timing"),
    }
    // Frames with sidecars of their own, which the window holds standing
    // in from the index's rows: "before" sends none of them as
    // neighbors, as the window did, and "after" sends them to be read
    // first. Each open goes under its sidecar's edit, as the window's
    // own read gives it.
    // A raw with no sidecar is given one under a warm white, so its base
    // is not the default's: point the walk at a copy of the raws.
    let edited = std::env::var("GREYCARD_WALK_EDITED").ok();
    if edited.is_some() {
        for path in &p {
            if greycard_edit::Sidecar::find(path).is_none() {
                write_sidecar(path, white_at(6500.0), 0);
            }
        }
    }
    let mut times = Vec::new();
    for (i, path) in p.iter().enumerate() {
        let generation = i as u64 + 1;
        let sent = Instant::now();
        let open = match &edited {
            Some(_) => {
                let (sidecar, _) = greycard_edit_io::on_disk::read_sidecar_said(path);
                Job::Open {
                    path: path.clone(),
                    version: None,
                    edit: sidecar.line.current().clone(),
                    generation,
                    seed_blend: false,
                    turn: sidecar.turn,
                    stance_known: false,
                    learned: Vec::new(),
                }
            }
            None => open_default(path, generation),
        };
        worker.send(open);
        let near = [
            (i + 1, Side::Next, 1),
            (i.wrapping_sub(1), Side::Previous, 1),
            (i + 2, Side::Next, 2),
            (i + 3, Side::Next, 3),
        ];
        worker.neighbors(
            generation,
            Heading::steady(Side::Next),
            near.iter()
                .filter_map(|&(at, side, steps)| match edited.as_deref() {
                    Some("before") => None,
                    Some(_) => p.get(at).map(|q| Neighbor {
                        steps,
                        ..read_first_at(q, None, side)
                    }),
                    None => p.get(at).map(|q| deep_at(q, side, steps)),
                })
                .collect(),
        );
        loop {
            let got = rx
                .recv_timeout(std::time::Duration::from_secs(600))
                .expect("a develop");
            if got == generation {
                break;
            }
        }
        let ms = sent.elapsed().as_secs_f64() * 1000.0;
        let name = super::engine::file_label(path);
        let shelved = super::prefetch::said(&name)
            .iter()
            .any(|l| *l == format!("the base of {name} from the shelf"));
        eprintln!(
            "step {i}: {name} {ms:.0} ms{}",
            if shelved { ", base from the shelf" } else { "" }
        );
        times.push(ms);
        std::thread::sleep(std::time::Duration::from_millis(gaps[i % gaps.len()]));
    }
    let steps = &times[1.min(times.len())..];
    eprintln!(
        "mean of {} steps after the first: {:.0} ms",
        steps.len(),
        steps.iter().sum::<f64>() / steps.len().max(1) as f64
    );
    for path in &p {
        for line in super::prefetch::said(&super::engine::file_label(path)) {
            if line.starts_with("prefetched") || line.contains("abandoned") {
                eprintln!("  {line}");
            }
        }
    }
    worker.stop();
}

/// A worker that opens no frame (an export's from the command line)
/// prefetches nothing, even when handed neighbors.
#[test]
fn a_worker_that_opens_nothing_prefetches_nothing() {
    use super::prefetch::{QUIET, said};
    let (dir, paths) = dngs("prefetch-none", &["pf-none-B.dng"], 64, 48);
    let worker = Worker::new(|_| {});
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![neighbor_at(&paths[0], Edit::default(), Side::Next)],
    );
    std::thread::sleep(QUIET * 10);
    assert!(said("pf-none-").is_empty());
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// The worker's prefetch against the real queue, held at a check
/// between two stages while a request is sent: the open of the very
/// frame being made has it go on, and that open lands from the shelf;
/// a newer open, with its own neighbors, has the one in hand give way,
/// and the older list is not taken up again; a panic in a prefetch
/// lets the rest of its list go, and the worker goes on.
#[test]
fn the_worker_s_prefetch_meets_the_requests_of_the_real_queue() {
    use super::prefetch::{AtCheck, QUIET, at_next_check, said};
    let names = [
        "pf-q-A.dng",
        "pf-q-B.dng",
        "pf-q-C.dng",
        "pf-q-D.dng",
        "pf-q-E.dng",
        "pf-q-F.dng",
        "pf-q-G.dng",
        "pf-q-H.dng",
    ];
    let (dir, p) = dngs("prefetch-queue", &names, 96, 64);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed { generation, .. } | Outcome::Failed { generation, .. } = o {
            tx.lock().unwrap().send(generation).unwrap();
        }
    });
    let wait = std::time::Duration::from_secs(60);
    let open = |path: &PathBuf, generation: u64| {
        worker.send(Job::Open {
            path: path.clone(),
            version: None,
            edit: Edit::default(),
            generation,
            seed_blend: false,
            turn: 0,
            stance_known: false,
            learned: Vec::new(),
        });
    };
    let landed = |generation: u64| {
        assert_eq!(rx.recv_timeout(wait).expect("a develop lands"), generation);
    };
    let until = |what: &str, seen: &dyn Fn() -> bool| {
        let asked = Instant::now();
        while !seen() {
            assert!(asked.elapsed() < wait, "{what}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let said_one = |name: &str, line: &str| said(name).iter().any(|l| l.contains(line));
    let hold = |path: &PathBuf| {
        let (at_tx, at_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel();
        at_next_check(path, AtCheck::Hold(at_tx, go_rx));
        (at_rx, go_tx)
    };

    // B's open sent while B's prefetch is held before its prepare.
    let (at, go) = hold(&p[1]);
    open(&p[0], 1);
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![neighbor_at(&p[1], Edit::default(), Side::Next)],
    );
    landed(1);
    assert_eq!(at.recv_timeout(wait).expect("held"), "prepare");
    open(&p[1], 2);
    go.send(()).unwrap();
    landed(2);
    assert!(said_one(
        "pf-q-B.dng",
        "the prefetch of pf-q-B.dng goes on before its prepare: it is the frame being opened"
    ));
    assert!(said_one(
        "pf-q-B.dng",
        "prefetched pf-q-B.dng, the next frame"
    ));
    assert!(said_one(
        "pf-q-B.dng",
        "the base of pf-q-B.dng from the shelf"
    ));

    // C's prefetch, for B's open, held; E opened with neighbors of its
    // own: C gives way, D is made as E's next and never as B's previous.
    let (at, go) = hold(&p[2]);
    worker.neighbors(
        2,
        Heading::steady(Side::Next),
        vec![
            neighbor_at(&p[2], Edit::default(), Side::Next),
            neighbor_at(&p[3], Edit::default(), Side::Previous),
        ],
    );
    assert_eq!(at.recv_timeout(wait).expect("held"), "prepare");
    open(&p[4], 3);
    worker.neighbors(
        3,
        Heading::steady(Side::Next),
        vec![neighbor_at(&p[3], Edit::default(), Side::Next)],
    );
    go.send(()).unwrap();
    landed(3);
    until("D made for E", &|| {
        said_one("pf-q-D.dng", "prefetched pf-q-D.dng, the next frame")
    });
    assert!(said_one(
        "pf-q-C.dng",
        "the prefetch of the next frame, pf-q-C.dng, abandoned before its prepare"
    ));
    assert!(!said_one("pf-q-C.dng", "prefetched"), "C never made");
    assert!(
        !said_one("pf-q-D.dng", "the previous frame"),
        "B's list let go"
    );

    // G's prefetch panics: H, after it in the list, is let go.
    at_next_check(&p[6], AtCheck::Panic);
    open(&p[5], 4);
    worker.neighbors(
        4,
        Heading::steady(Side::Next),
        vec![
            neighbor_at(&p[6], Edit::default(), Side::Next),
            neighbor_at(&p[7], Edit::default(), Side::Previous),
        ],
    );
    landed(4);
    until("G panicked", &|| {
        said_one("pf-q-G.dng", "the worker panicked")
    });
    std::thread::sleep(QUIET * 20);
    assert!(said("pf-q-H.dng").is_empty(), "H let go");
    open(&p[7], 5);
    landed(5);
    assert!(!said_one("pf-q-H.dng", "from the shelf"));
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// The editor left for culling or the grid (no frame open): the
/// neighbors waiting are let go, and a prefetch is not begun for them.
#[test]
fn leaving_the_editor_lets_the_neighbors_go() {
    let worker = Worker::new(|_| {});
    // A frame open whose develop never lands: the list waits for it.
    worker.set_open_frame(Some(Frame::file("/nowhere/pf-left-A.CR3")));
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![neighbor_at(
            std::path::Path::new("/nowhere/pf-left-B.CR3"),
            Edit::default(),
            Side::Next,
        )],
    );
    assert!(worker.queue.0.lock().unwrap().neighbors.is_some());
    worker.set_open_frame(None);
    assert!(worker.queue.0.lock().unwrap().neighbors.is_none());
    worker.stop();
}

/// The frame still open when the editor comes back from culling or the
/// grid has its neighbors sent again: taken for the open whose develop
/// landed last, on that file, or in place of that open's own list, and
/// never over another open's; the worker says which open that is once
/// its develop lands.
#[test]
fn the_neighbors_come_back_with_the_open_frame() {
    let (dir, paths) = dngs("prefetch-back", &["pf-back-A.dng"], 64, 48);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed { generation, .. } | Outcome::Failed { generation, .. } = o {
            tx.lock().unwrap().send(generation).unwrap();
        }
    });
    let b = || {
        vec![neighbor_at(
            std::path::Path::new("/nowhere/pf-back-B.CR3"),
            Edit::default(),
            Side::Next,
        )]
    };
    // Nothing landed yet: nothing taken.
    assert!(!worker.neighbors_again(7, &paths[0], Heading::steady(Side::Next), b(), "a test"));
    assert!(worker.queue.0.lock().unwrap().neighbors.is_none());
    worker.send(Job::Open {
        path: paths[0].clone(),
        version: None,
        edit: Edit::default(),
        generation: 7,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let wait = std::time::Duration::from_secs(60);
    assert_eq!(rx.recv_timeout(wait).expect("the open lands"), 7);
    assert_eq!(
        worker.queue.0.lock().unwrap().landed,
        Some((7, paths[0].clone()))
    );
    // Away to the grid: let go. Back on another file: not taken.
    worker.set_open_frame(None);
    assert!(!worker.neighbors_again(
        7,
        std::path::Path::new("/nowhere/other.CR3"),
        Heading::steady(Side::Next),
        b(),
        "a test"
    ));
    assert!(worker.queue.0.lock().unwrap().neighbors.is_none());
    // Back on A: taken under A's open.
    worker.set_open_frame(Some(Frame::file(paths[0].clone())));
    assert!(worker.neighbors_again(7, &paths[0], Heading::steady(Side::Next), b(), "a test"));
    {
        let q = worker.queue.0.lock().unwrap();
        let n = q.neighbors.as_ref().expect("sent again");
        assert_eq!(n.generation, 7);
    }
    // Sent again for the same open (the filter or the order moved):
    // its list, waiting or done, is replaced.
    assert!(worker.neighbors_again(
        7,
        &paths[0],
        Heading::steady(Side::Next),
        Vec::new(),
        "a test"
    ));
    // A list for another open waiting already is not replaced.
    worker.neighbors(9, Heading::steady(Side::Next), b());
    assert!(!worker.neighbors_again(7, &paths[0], Heading::steady(Side::Next), b(), "a test"));
    assert_eq!(
        worker
            .queue
            .0
            .lock()
            .unwrap()
            .neighbors
            .as_ref()
            .map(|n| n.generation),
        Some(9)
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A neighbor's edit changed after its base was prefetched (a sync, a
/// paste): a change after the base keeps the base on the shelf, and the
/// neighbor sent again under it is not made twice; a change to the base
/// drops it, said, and the neighbor sent again under the new edit is
/// made afresh at the next idle, which its open then takes.
#[test]
fn a_neighbor_whose_edit_changed_is_dropped_and_made_again() {
    use super::prefetch::said;
    let (dir, paths) = dngs("prefetch-edit", &["pf-ed-A.dng", "pf-ed-B.dng"], 96, 64);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed { generation, .. } | Outcome::Failed { generation, .. } = o {
            tx.lock().unwrap().send(generation).unwrap();
        }
    });
    let wait = std::time::Duration::from_secs(60);
    let until = |what: &str, done: &dyn Fn() -> bool| {
        let asked = Instant::now();
        while !done() {
            assert!(asked.elapsed() < wait, "{what}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    let count = |starts: &str| {
        said("pf-ed-B.dng")
            .iter()
            .filter(|l| l.starts_with(starts))
            .count()
    };
    let open = |path: &PathBuf, edit: Edit, generation: u64| Job::Open {
        path: path.clone(),
        version: None,
        edit,
        generation,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    };
    worker.set_open_frame(Some(Frame::file(paths[0].clone())));
    worker.send(open(&paths[0], Edit::default(), 1));
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![neighbor_at(&paths[1], Edit::default(), Side::Next)],
    );
    assert_eq!(rx.recv_timeout(wait).expect("the open lands"), 1);
    until("B prefetched", &|| count("prefetched pf-ed-B.dng") == 1);

    // After the base: kept, and not made again.
    let mut brighter = Edit::default();
    brighter.light.exposure = 1.0;
    worker.edit_changed(paths[1].clone(), Some(holds(&brighter)));
    assert!(worker.neighbors_again(
        1,
        &paths[0],
        Heading::steady(Side::Next),
        vec![neighbor_at(&paths[1], brighter, Side::Next)],
        "a neighbor's edit changed"
    ));
    until("the resent B skipped", &|| {
        said("pf-ed-B.dng")
            .iter()
            .any(|l| l.ends_with("skipped: its base is on the shelf"))
    });
    assert_eq!(count("shelved base of pf-ed-B.dng dropped"), 0);

    // The base itself: dropped, and made again under the new edit.
    let warmer = Edit {
        white_balance: greycard_edit::WhiteBalance::Custom {
            temperature: 6500.0,
            tint: 0.0,
        },
        ..Edit::default()
    };
    worker.edit_changed(paths[1].clone(), Some(holds(&warmer)));
    until("B's base dropped", &|| {
        said("pf-ed-B.dng")
            .iter()
            .any(|l| l == "shelved base of pf-ed-B.dng dropped: edit changed")
    });
    assert!(worker.neighbors_again(
        1,
        &paths[0],
        Heading::steady(Side::Next),
        vec![neighbor_at(&paths[1], warmer.clone(), Side::Next)],
        "a neighbor's edit changed"
    ));
    until("B prefetched again", &|| {
        count("prefetched pf-ed-B.dng") == 2
    });
    worker.set_open_frame(Some(Frame::file(paths[1].clone())));
    worker.send(open(&paths[1], warmer, 2));
    assert_eq!(rx.recv_timeout(wait).expect("B's open lands"), 2);
    assert_eq!(count("the base of pf-ed-B.dng from the shelf"), 1);
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// What a file holds now, as the window says it after a change: `edit`
/// alone, unturned.
fn holds(edit: &Edit) -> FileNow {
    FileNow {
        edits: vec![edit.clone()],
        turn: 0,
    }
}

/// A neighbor's edit changed while its prefetch is in hand: a change
/// after the base does not stop it; a change to the base has it give
/// way, and it is let go rather than put back on the list the window
/// sent again under the new edit, so only the new base is made, and
/// its open takes it.
#[test]
fn a_prefetch_whose_edit_changes_under_it_gives_way_and_is_let_go() {
    use super::prefetch::{AtCheck, at_next_check, said};
    let (dir, paths) = dngs("prefetch-moved", &["pf-mv-A.dng", "pf-mv-B.dng"], 96, 64);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed { generation, .. } | Outcome::Failed { generation, .. } = o {
            tx.lock().unwrap().send(generation).unwrap();
        }
    });
    let wait = std::time::Duration::from_secs(60);
    let until = |what: &str, done: &dyn Fn() -> bool| {
        let asked = Instant::now();
        while !done() {
            assert!(asked.elapsed() < wait, "{what}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    let count = |starts: &str| {
        said("pf-mv-B.dng")
            .iter()
            .filter(|l| l.starts_with(starts))
            .count()
    };
    let hold = |path: &PathBuf| {
        let (at_tx, at_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel();
        at_next_check(path, AtCheck::Hold(at_tx, go_rx));
        (at_rx, go_tx)
    };
    let open = |path: &PathBuf, edit: Edit, generation: u64| Job::Open {
        path: path.clone(),
        version: None,
        edit,
        generation,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    };
    let warmer = Edit {
        white_balance: greycard_edit::WhiteBalance::Custom {
            temperature: 6500.0,
            tint: 0.0,
        },
        ..Edit::default()
    };
    let mut brighter = Edit::default();
    brighter.light.exposure = 1.0;

    // Held before its prepare, an exposure synced onto it: it goes on.
    let (at, go) = hold(&paths[1]);
    worker.set_open_frame(Some(Frame::file(paths[0].clone())));
    worker.send(open(&paths[0], Edit::default(), 1));
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![neighbor_at(&paths[1], Edit::default(), Side::Next)],
    );
    assert_eq!(rx.recv_timeout(wait).expect("the open lands"), 1);
    assert_eq!(at.recv_timeout(wait).expect("held"), "prepare");
    worker.edit_changed(paths[1].clone(), Some(holds(&brighter)));
    go.send(()).unwrap();
    until("B prefetched", &|| count("prefetched pf-mv-B.dng") == 1);
    assert_eq!(
        count("the prefetch of the next frame, pf-mv-B.dng, abandoned"),
        0
    );

    // Its base dropped by a white balance pasted onto it, and B sent
    // again under it; the new prefetch held, and a second paste, back
    // to the default, comes meanwhile: it gives way and is let go.
    worker.edit_changed(paths[1].clone(), Some(holds(&warmer)));
    until("B's base dropped", &|| {
        count("shelved base of pf-mv-B.dng dropped") == 1
    });
    let (at, go) = hold(&paths[1]);
    assert!(worker.neighbors_again(
        1,
        &paths[0],
        Heading::steady(Side::Next),
        vec![neighbor_at(&paths[1], warmer.clone(), Side::Next)],
        "a neighbor's edit changed"
    ));
    assert_eq!(at.recv_timeout(wait).expect("held"), "prepare");
    worker.edit_changed(paths[1].clone(), Some(holds(&Edit::default())));
    assert!(worker.neighbors_again(
        1,
        &paths[0],
        Heading::steady(Side::Next),
        vec![neighbor_at(&paths[1], Edit::default(), Side::Next)],
        "a neighbor's edit changed"
    ));
    go.send(()).unwrap();
    until("B made again under the default", &|| {
        count("prefetched pf-mv-B.dng") == 2
    });
    assert_eq!(
        count("the prefetch of the next frame, pf-mv-B.dng, abandoned before its prepare"),
        1
    );
    assert_eq!(
        count("the prefetch of pf-mv-B.dng let go: its edit changed"),
        1
    );
    // Not made a third time under the warmer edit it gave way under.
    std::thread::sleep(super::prefetch::QUIET * 20);
    assert_eq!(count("prefetched pf-mv-B.dng"), 2);
    worker.set_open_frame(Some(Frame::file(paths[1].clone())));
    worker.send(open(&paths[1], Edit::default(), 2));
    assert_eq!(rx.recv_timeout(wait).expect("B's open lands"), 2);
    assert_eq!(count("the base of pf-mv-B.dng from the shelf"), 1);
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A device lost under a develop's ops (destroyed here, as a driver's
/// reset loses it) costs no panic: each op's error sends it to the
/// CPU, the context goes, and the develop lands a picture made there.
#[test]
fn a_develop_on_a_lost_device_lands_on_the_cpu() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the lost device's develop");
        return;
    };
    ctx.device().destroy();
    let frame = Arc::new(aberrated_frame(400, 320));
    let mut gpu = Some(ctx);
    let mut base = None;
    let (outcome, image) = develop_once(&frame, &mut base, &mut gpu);
    assert!(
        matches!(
            outcome,
            Outcome::Developed {
                image: Developed::Halves(_),
                ..
            }
        ),
        "a picture from the CPU"
    );
    assert!(image.is_some());
    assert!(gpu.is_none(), "the lost context let go");
    assert!(!base.as_ref().unwrap().on_gpu);
}

/// A loss found after the develop landed (in a save's read back, by
/// the window's callback): the context goes, and the develop whose
/// texture is on the lost device is asked again, under its own
/// generation, which then lands on the CPU. Told the editor starts
/// again, nothing is asked again.
#[test]
fn a_loss_after_the_develop_develops_it_again_on_the_cpu() {
    for again in [true, false] {
        let Some(ctx) = greycard_gpu::Context::own().ok() else {
            skipped("the develop again after a loss");
            return;
        };
        let frame = Arc::new(aberrated_frame(400, 320));
        let mut gpu = Some(ctx);
        let mut base = None;
        let (outcome, _) = develop_once(&frame, &mut base, &mut gpu);
        let mut shown = ShownTexture::of(&Edit::default(), 0, &outcome, base.as_ref());
        assert!(shown.is_some(), "the develop left a texture on the device");
        gpu.as_ref().unwrap().device().destroy();
        let redo = super::engine::lose_gpu(&mut gpu, &mut base, &mut shown, again);
        assert!(gpu.is_none() && shown.is_none());
        let b = base.as_ref().unwrap();
        assert!(!b.kept.has_upload() && !b.kept.on_device(Keep::PreSharpen));
        if !again {
            assert!(redo.is_none());
            continue;
        }
        let Some(Job::Develop {
            edit,
            generation,
            turn,
            ..
        }) = redo
        else {
            panic!("the develop asked again");
        };
        assert_eq!((generation, turn), (1, 0));
        assert!(edit == Edit::default());
        let (outcome, _) = develop_once(&frame, &mut base, &mut gpu);
        assert!(matches!(
            outcome,
            Outcome::Developed {
                image: Developed::Halves(_),
                generation: 1,
                ..
            }
        ));
    }
}

/// A drag that moves the base (a white balance, a lens) asks for its
/// masks once, after its last develop: each develop's asks of a new base
/// wait in place of the last's until the develops have been quiet a
/// while, and none is made between two develops of the drag.
#[test]
fn a_drag_s_new_bases_ask_for_their_masks_once_after_it() {
    let lock = Mutex::new(Queue::default());
    let sky = Shape::Sky { picks: Vec::new() };
    let start = Instant::now();
    let tick = std::time::Duration::from_millis(300);
    // Five develops of a drag, a third of a second apart, each from a
    // base the panel's mask was not made from.
    for i in 0..5 {
        let at = start + tick * i;
        super::engine::after_develop(&lock, (Vec::new(), vec![((1, 0), sky.clone(), None)]), at);
        let mut q = lock.lock().unwrap();
        super::engine::settle_due(&mut q, at + tick / 2);
        assert!(q.masks.is_empty(), "nothing made during the drag ({i})");
    }
    let last = start + tick * 4;
    {
        let mut q = lock.lock().unwrap();
        let left = masks::settling_left(&q.settling, last).unwrap();
        assert_eq!(left, masks::SETTLE, "due a settle after the last develop");
        super::engine::settle_due(&mut q, last + masks::SETTLE);
        let queued: Vec<_> = q.masks.iter().map(|(k, s, _)| (*k, s.clone())).collect();
        assert_eq!(queued, vec![((1, 0), sky.clone())], "one mask job");
        assert!(q.settling.is_none());
        q.masks.clear();
        super::engine::settle_due(&mut q, last + masks::SETTLE * 3);
        assert!(q.masks.is_empty(), "and only one");
    }
    // A develop back on the mask's own base asks nothing, and lets go of
    // what a drag's earlier develop held; a fill's moved patches are
    // asked for at once.
    super::engine::after_develop(&lock, (Vec::new(), vec![((1, 0), sky.clone(), None)]), last);
    super::engine::after_develop(&lock, (vec![((2, 0), sky.clone(), None)], Vec::new()), last);
    let mut q = lock.lock().unwrap();
    assert!(q.settling.is_none());
    assert_eq!(q.masks.len(), 1);
    super::engine::settle_due(&mut q, last + masks::SETTLE * 2);
    assert_eq!(q.masks.len(), 1);
}

/// A mask the worker asks for again behind a develop does not take the
/// place of one the panel queued for the same key, which is newer.
#[test]
fn a_mask_asked_again_leaves_a_queued_ask_of_the_same_key() {
    let lock = Mutex::new(Queue::default());
    let sky = Shape::Sky { picks: Vec::new() };
    lock.lock()
        .unwrap()
        .masks
        .push_back(((1, 0), sky.clone(), None));
    super::engine::ask_again(
        &lock,
        vec![
            ((1, 0), Shape::Subject {}, None),
            ((2, 0), Shape::Subject {}, None),
        ],
    );
    let q = lock.lock().unwrap();
    let queued: Vec<_> = q.masks.iter().map(|(k, s, _)| (*k, s.clone())).collect();
    assert_eq!(queued, vec![((1, 0), sky), ((2, 0), Shape::Subject {})]);
}

/// The models see the base: the mask cache's reduced develop of an
/// edit with a retouch (a Fill would refuse it, a heal would put the
/// patched picture's raster in the base's slot) is, to the bit, the
/// reduced develop of the same edit without its retouch, through the
/// Detail section.
#[test]
fn the_mask_cache_develops_without_the_retouch() {
    use crate::stages::Purpose;
    let dir = crate::testing::scratch_dir("models-see");
    let path = dir.join("frame.dng");
    crate::testing::linear_dng(&path, 96, 64, "Canon", "Canon EOS R5");
    let mut edit = Edit::default();
    edit.light.exposure = 0.7;
    edit.detail.clarity = 0.4;
    edit.retouch.patches.push(greycard_edit::retouch::Patch {
        id: 1,
        points: vec![[0.3, 0.3]],
        radius: 0.05,
        ..Default::default()
    });
    edit.retouch.patches.push(greycard_edit::retouch::Patch {
        id: 2,
        method: greycard_edit::retouch::Method::Fill,
        points: vec![[0.5, 0.3]],
        radius: 0.05,
        ..Default::default()
    });
    let seen = crate::reduced::develop_as(&path, &edit, 0, None, &Purpose::ModelsSee)
        .expect("the models' picture develops");
    let bare = Edit {
        retouch: Default::default(),
        ..edit
    };
    let reduced = crate::reduced::develop_as(&path, &bare, 0, None, &Purpose::Reduced)
        .expect("the bare edit develops reduced");
    assert_eq!((seen.image.width, seen.image.height), (96, 64));
    assert!(seen.image.data == reduced.image.data);
    assert_eq!((seen.full, seen.kind), (reduced.full, reduced.kind));
    crate::testing::remove_dir_retry(&dir);
}

/// A retouch as the develop applied it is the edit's when the only
/// difference is a source the develop chose.
#[test]
fn a_retouch_applied_is_the_edits_with_the_sources_chosen() {
    use greycard_edit::retouch::Patch;
    let heal = Patch {
        id: 1,
        points: vec![[0.2, 0.2]],
        ..Default::default()
    };
    let edit = Retouch {
        enabled: true,
        patches: vec![heal.clone()],
    };
    let applied = Retouch {
        enabled: true,
        patches: vec![Patch {
            source: Some([0.1, 0.0]),
            ..heal.clone()
        }],
    };
    assert!(masks::same_retouch(&edit, &applied));
    let moved = Retouch {
        enabled: true,
        patches: vec![Patch {
            points: vec![[0.3, 0.2]],
            source: Some([0.1, 0.0]),
            ..heal
        }],
    };
    assert!(!masks::same_retouch(&edit, &moved));
    assert!(!masks::same_retouch(&edit, &Retouch::default()));
}

/// A light field with a dark bar down its middle, and an edit that
/// clones the bar away from the field beside it.
fn bar_and_its_clone() -> (Arc<WorkingImage>, Edit) {
    let n = 256;
    let mut image = WorkingImage::new(n, n);
    for (i, px) in image.data.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        let dark = (120..136).contains(&(i % n));
        px.fill(if dark { 0.02 } else { 0.4 });
    }
    let edit = Edit {
        retouch: Retouch {
            enabled: true,
            patches: vec![greycard_edit::retouch::Patch {
                id: 1,
                method: greycard_edit::retouch::Method::Clone,
                points: vec![[0.5, 0.05], [0.5, 0.95]],
                radius: 0.06,
                source: Some([0.25, 0.0]),
                ..Default::default()
            }],
        },
        ..Default::default()
    };
    (Arc::new(image), edit)
}

/// The guide plane is the retouched picture's: a dark bar cloned
/// away leaves no trace in it for the tone sliders to lift, and an
/// unchanged retouch keeps the plane rather than making it again.
#[test]
fn the_guide_is_made_after_the_retouch() {
    let (image, edit) = bar_and_its_clone();
    let input = picture_input(&image);
    let mut base = None;
    develop_picture(&input, &Edit::default(), &mut base, &mut None);
    let before = base.as_ref().unwrap().guide.clone();
    let (bar, beside) = ((128.0, 128.0), (200.0, 128.0));
    assert!(
        before.at(beside.0, beside.1) - before.at(bar.0, bar.1) > 1.0,
        "the bar is in the base's guide"
    );
    develop_picture(&input, &edit, &mut base, &mut None);
    let b = base.as_ref().unwrap();
    let patched = b.kept.patched().expect("the retouch is kept").image;
    let px = &patched.data[(128 * 256 + 128) * 3..][..3];
    assert!((px[1] - 0.4).abs() < 1e-3, "the clone took the bar: {px:?}");
    let at = (b.guide.at(bar.0, bar.1), b.guide.at(beside.0, beside.1));
    assert!(
        (at.0 - at.1).abs() < 0.02,
        "the bar's ghost in the guide: {at:?}"
    );
    let made = b.guide.clone();
    develop_picture(&input, &edit, &mut base, &mut None);
    assert!(Arc::ptr_eq(&made, &base.as_ref().unwrap().guide));
}

/// Clearing the retouch gives the guide back to the base.
#[test]
fn a_cleared_retouch_brings_back_the_bases_guide() {
    let (image, edit) = bar_and_its_clone();
    let input = picture_input(&image);
    let mut base = None;
    develop_picture(&input, &Edit::default(), &mut base, &mut None);
    let own = base.as_ref().unwrap().guide.data.clone();
    develop_picture(&input, &edit, &mut base, &mut None);
    assert_ne!(base.as_ref().unwrap().guide.data, own);
    develop_picture(&input, &Edit::default(), &mut base, &mut None);
    let b = base.as_ref().unwrap();
    assert!(b.kept.patched().is_none());
    assert_eq!(b.guide.data, own);
}

/// What the open file's export writes for `input` under `edit` at
/// `turn`, walked from the window's develop of it, and what a set
/// frame's walk of its stage list writes: each picture's pixels with
/// what it went without, with no model in the store for either.
fn develop_and_walk(
    what: &str,
    input: &Input,
    edit: &Edit,
    turn: u8,
) -> (
    (greycard_render::export::Rendered, Vec<String>),
    (greycard_render::export::Rendered, Vec<String>),
) {
    let empty = crate::testing::scratch_dir(what).join("models");
    std::fs::create_dir_all(&empty).unwrap();
    let settings = greycard_render::export::Settings {
        format: greycard_render::export::Format::Tiff,
        ..Default::default()
    };
    let deliver: Deliver = Arc::new(|_| {});
    let mut ai = Ai::for_test(greycard_ai::Store::at(&empty), Vec::new());
    let mut base = None;
    develop_job(
        input, edit, turn, 0, &mut base, &mut ai, None, None, &mut None, &deliver,
    );
    let (developed, said) = super::export::render_open(
        edit,
        turn,
        input,
        base.as_ref(),
        &mut WorkerModels::new(&mut ai, None, &deliver, 0),
        None,
        &mut None,
        &settings,
    )
    .unwrap();
    let mut ai = Ai::for_test(greycard_ai::Store::at(&empty), Vec::new());
    let walked = super::export::walk_export(
        input.clone(),
        edit,
        turn,
        &mut super::base::WorkerModels::new(&mut ai, None, &deliver, 0),
        None,
        &mut None,
        &settings,
    )
    .unwrap();
    crate::testing::remove_dir_retry(empty.parent().unwrap());
    ((developed, said), walked)
}

fn same_pixels(a: &greycard_render::export::Rendered, b: &greycard_render::export::Rendered) {
    use greycard_render::export::Pixels;
    assert_eq!((a.width, a.height), (b.width, b.height));
    match (&a.pixels, &b.pixels) {
        (Pixels::Sixteen(a), Pixels::Sixteen(b)) => assert!(a == b, "the pixels differ"),
        (Pixels::Eight(a), Pixels::Eight(b)) => assert!(a == b, "the pixels differ"),
        _ => panic!("one picture is 8-bit and the other 16"),
    }
}

/// An edit that asks for a learned denoiser, a Fill, and a learned
/// mask, none of whose models is in the store, beside a Heal, a
/// Detail of the edit's and the mask's own, a mask refined against the
/// picture, the sharpen, and a straightened crop.
fn edit_without_its_models() -> Edit {
    use greycard_edit::mask::Component;
    use greycard_edit::retouch::{Method, Patch};
    let mut edit = Edit::default();
    edit.noise.learned = greycard_edit::Learned::Fast;
    edit.noise.learned_strength = 0.6;
    edit.detail.clarity = 0.3;
    edit.geometry.angle = 2.5;
    edit.geometry.crop = Some(greycard_edit::geometry::Crop {
        x: 0.1,
        y: 0.05,
        w: 0.7,
        h: 0.8,
    });
    edit.retouch.patches = vec![
        Patch {
            id: 21,
            method: Method::Fill,
            points: vec![[0.5, 0.5]],
            radius: 0.08,
            ..Patch::default()
        },
        Patch {
            id: 22,
            method: Method::Heal,
            points: vec![[0.3, 0.6]],
            radius: 0.05,
            ..Patch::default()
        },
    ];
    let mut look = greycard_edit::Look::default();
    look.light.exposure = 0.8;
    look.detail.clarity = 0.5;
    edit.adjustments.push(greycard_edit::Adjustment {
        id: 11,
        name: "Face".into(),
        mask: greycard_edit::mask::Mask {
            components: vec![Component {
                shape: Shape::Subject {},
                ..Component::default()
            }],
            invert: false,
            ..Default::default()
        },
        look,
        ..greycard_edit::Adjustment::default()
    });
    // A mask refined against the picture: a hard edge across the frame
    // and a luminance window, its Refine edge on.
    let mut look = greycard_edit::Look::default();
    look.light.exposure = -0.7;
    edit.adjustments.push(greycard_edit::Adjustment {
        id: 12,
        name: "Refined".into(),
        mask: greycard_edit::mask::Mask {
            components: vec![
                Component {
                    shape: Shape::Linear {
                        from: [0.55, 0.3],
                        to: [0.549, 0.3],
                    },
                    ..Component::default()
                },
                Component {
                    shape: Shape::LUMINANCE,
                    mode: greycard_edit::mask::Mode::Intersect,
                    ..Component::default()
                },
            ],
            refine: 0.5,
            ..Default::default()
        },
        look,
        ..greycard_edit::Adjustment::default()
    });
    edit
}

/// A set frame's export walks its stage list and writes what the open
/// file's develop and finish write: the same pixels, and the same words
/// for what it went without, the engine's denoise standing in for the
/// learned one, the Fill left as it was, and the mask written without
/// its shape; the refined mask refined on both. At each quarter turn,
/// the crop straightened.
#[test]
fn a_set_frame_s_walk_writes_what_the_develop_writes() {
    let frame = Arc::new(aberrated_frame(160, 120));
    let edit = edit_without_its_models();
    for turn in 0..4 {
        let ((developed, said), (walked, left_out)) =
            develop_and_walk("export-walk", &Input::Raw(frame.clone()), &edit, turn);
        same_pixels(&developed, &walked);
        assert_eq!(left_out, said);
        assert_eq!(left_out.len(), 3, "{left_out:?}");
        assert!(
            left_out[0].starts_with("the learned denoiser"),
            "{}",
            left_out[0]
        );
        assert!(left_out[1].starts_with("fill Fill 21: "), "{}", left_out[1]);
        assert!(
            left_out[2].starts_with("Face's Subject shape: "),
            "{}",
            left_out[2]
        );
        // The refined mask is refined on both: unrefined, another picture.
        if turn == 0 {
            let mut plain = edit.clone();
            plain.adjustments[1].mask.refine = 0.0;
            let ((unrefined, _), _) =
                develop_and_walk("export-walk-plain", &Input::Raw(frame.clone()), &plain, 0);
            use greycard_render::export::Pixels;
            let differs = match (&unrefined.pixels, &walked.pixels) {
                (Pixels::Sixteen(a), Pixels::Sixteen(b)) => a != b,
                (Pixels::Eight(a), Pixels::Eight(b)) => a != b,
                _ => true,
            };
            assert!(differs, "the refine did nothing");
        }
    }
}

/// An X-Trans mosaic asks for its tier's X-Trans network: not in the
/// store, the walk names that network as the develop does, stands the
/// engine's path in, and writes its picture.
#[test]
fn a_set_frame_s_walk_asks_for_the_x_trans_network() {
    let mut frame = aberrated_frame(168, 120);
    frame.layout = greycard_core::raw::SensorLayout::Cfa(greycard_core::raw::CfaPattern::xtrans());
    let mut edit = Edit::default();
    edit.noise.learned = greycard_edit::Learned::Fast;
    let ((developed, said), (walked, left_out)) =
        develop_and_walk("export-walk-xtrans", &Input::Raw(Arc::new(frame)), &edit, 0);
    same_pixels(&developed, &walked);
    assert_eq!(left_out, said);
    // Where ONNX Runtime cannot be had, the runtime is why, as on Bayer.
    let want = match greycard_ai::runtime::status() {
        Ok(_) => format!(
            "the learned denoiser: {} is not downloaded; the engine's denoise instead",
            greycard_ai::DENOISE_XTRANS_FAST.name
        ),
        Err(why) => {
            format!("the learned denoiser failed ({why}); the engine's denoise instead")
        }
    };
    assert_eq!(left_out, [want]);
}

/// A picture file walks as its develop does: turned, cropped, its
/// retouch and its mask, with no capture sharpen.
#[test]
fn a_set_picture_s_walk_writes_what_the_develop_writes() {
    let frame = Arc::new(aberrated_frame(160, 120));
    let mut base = None;
    let (_, image) = develop_once(&frame, &mut base, &mut None);
    let input = picture_input(&image.unwrap());
    let mut edit = edit_without_its_models();
    edit.noise.learned = greycard_edit::Learned::Off;
    edit.sharpen.enabled = false;
    for turn in [0, 3] {
        let ((developed, said), (walked, left_out)) =
            develop_and_walk("export-walk-picture", &input, &edit, turn);
        same_pixels(&developed, &walked);
        assert_eq!(left_out, said);
        assert_eq!(left_out.len(), 2, "{left_out:?}");
    }
}

/// An edit that refines one mask, a luminance window of 60 to 80 over
/// the frame, with Dehaze at `dehaze` and no capture sharpen, so the
/// picture the develop hands the finish is the one before the sharpen.
fn refining(dehaze: f32) -> Edit {
    use greycard_edit::mask::Component;
    let mut edit = Edit::default();
    edit.sharpen.enabled = false;
    edit.detail.dehaze = dehaze;
    edit.adjustments.push(greycard_edit::Adjustment {
        id: 3,
        mask: greycard_edit::mask::Mask {
            components: vec![Component {
                shape: Shape::Luminance {
                    low: 0.6,
                    high: 0.8,
                    low_feather: 0.05,
                    high_feather: 0.05,
                },
                ..Component::default()
            }],
            refine: 0.01,
            ..Default::default()
        },
        ..greycard_edit::Adjustment::default()
    });
    edit
}

/// The develop of an edit that refines a mask hands the window the
/// picture its refined masks are made against, made from the picture
/// the finish's range shapes read, the patched one from before the
/// Dehaze: the same picture on the masks' grid, where the developed
/// picture after the Dehaze is another. And a develop that refines
/// nothing makes none and keeps none.
#[test]
fn the_refined_masks_picture_is_the_one_the_range_shapes_read() {
    let frame = Arc::new(aberrated_frame(160, 120));
    let mut base = None;
    let deliver: Deliver = Arc::new(|_| {});
    let edit = refining(0.4);
    let (outcome, image) = develop_job(
        &Input::Raw(frame.clone()),
        &edit,
        0,
        1,
        &mut base,
        &mut Ai::new(),
        None,
        None,
        &mut None,
        &deliver,
    );
    let Outcome::Developed { guide, .. } = outcome else {
        panic!("developed")
    };
    let picture = guide.picture.clone().expect("a picture beside the guide");
    let developed = image.unwrap();
    assert!(*picture != greycard_render::refine::mask_picture(&developed));
    let b = base.as_ref().unwrap();
    let patched = b.patched_picture();
    assert!(*picture == greycard_render::refine::mask_picture(&patched));
    assert!(b.mask_picture.is_some());
    // The open frame's own pictures (`keep`) are made against it too.
    assert!(b.mask_picture.is_some());
    // A Luminance shape reads the same lightness in the picture refined
    // against as the finish reads in the patched one, on average over
    // each texel, to the half float's rounding.
    let developed = patched.clone();
    let (w, h) = (developed.width, developed.height);
    let texel = |gx: usize, gy: usize| picture.lab[gy * picture.width + gx][0].to_f32();
    let (gx, gy) = (picture.width / 2, picture.height / 3);
    let (x, y) = (gx * w / picture.width, gy * h / picture.height);
    let read = greycard_render::finish::oklab(developed.pixel(x, y))[0];
    assert!(
        (texel(gx, gy) - read).abs() < 3e-3,
        "{} {read}",
        texel(gx, gy)
    );
    // Refining nothing: no picture made, and the kept one let go.
    let mut plain = edit.clone();
    plain.adjustments[0].mask.refine = 0.0;
    let (outcome, _) = develop_job(
        &Input::Raw(frame),
        &plain,
        0,
        2,
        &mut base,
        &mut Ai::new(),
        None,
        None,
        &mut None,
        &deliver,
    );
    let Outcome::Developed { guide, .. } = outcome else {
        panic!("developed")
    };
    assert!(guide.picture.is_none());
    assert!(base.as_ref().unwrap().mask_picture.is_none());
    // A save of the refining edit with no develop between (a Refine
    // edge moved from 0, `keep_open`) makes it from the base in hand:
    // the picture the develop made, and kept for the next.
    let b = base.as_mut().unwrap();
    let made = masks::picture_for(b, &edit).expect("made for the save");
    assert!(*made == *picture);
    assert!(b.mask_picture.is_some());
    assert!(masks::picture_for(b, &plain).is_none());
}

/// A move after the patched picture (the Detail section, the sharpen)
/// taps the refined masks' picture no more: the one kept is handed out
/// again.
#[test]
fn a_sharpen_move_keeps_the_refined_masks_picture() {
    let frame = Arc::new(aberrated_frame(160, 120));
    let deliver: Deliver = Arc::new(|_| {});
    let mut base = None;
    let mut develop = |edit: &Edit| {
        let (outcome, _) = develop_job(
            &Input::Raw(frame.clone()),
            edit,
            0,
            1,
            &mut base,
            &mut Ai::new(),
            None,
            None,
            &mut None,
            &deliver,
        );
        let Outcome::Developed { guide, .. } = outcome else {
            panic!("developed")
        };
        guide.picture.clone().expect("a picture beside the guide")
    };
    let mut edit = refining(0.4);
    edit.sharpen.enabled = true;
    let first = develop(&edit);
    edit.sharpen.iterations += 1;
    assert!(Arc::ptr_eq(&develop(&edit), &first), "kept");
    // Made before the Detail section, so a Dehaze move keeps it too,
    // for a range shape and a gradient alike; and a gradient's is the
    // same picture, whatever shape another mask holds.
    edit.detail.dehaze = 0.6;
    assert!(
        Arc::ptr_eq(&develop(&edit), &first),
        "kept by a Dehaze move"
    );
    edit.adjustments[0].mask.components[0] = greycard_edit::mask::Component::default();
    assert!(Arc::ptr_eq(&develop(&edit), &first), "kept for a gradient");
}

/// A set frame's walk makes the refined masks' picture after the
/// Dehaze, as the develop does: the two export the same pixels with
/// Dehaze on, and a walk that refines nothing has no such step.
#[test]
fn a_walk_refines_against_the_picture_after_the_dehaze() {
    let frame = Arc::new(aberrated_frame(160, 120));
    let edit = refining(0.4);
    let ((developed, _), (walked, _)) =
        develop_and_walk("export-walk-refined", &Input::Raw(frame), &edit, 0);
    same_pixels(&developed, &walked);
    let facts = Input::Raw(Arc::new(aberrated_frame(160, 120)));
    let purpose = crate::stages::Purpose::Export(Box::default());
    let has_tap = |edit: &Edit| {
        let input = match &facts {
            Input::Raw(f) => crate::walk::Input::Raw(f.clone()),
            Input::Picture { .. } => unreachable!(),
        };
        crate::stages::stages(edit, &purpose, &input.facts(0), None)
            .steps
            .iter()
            .any(|s| matches!(s, crate::stages::Step::Tap(crate::stages::Tap::MaskPicture)))
    };
    assert!(has_tap(&edit));
    let mut plain = edit.clone();
    plain.adjustments[0].mask.refine = 0.0;
    assert!(!has_tap(&plain));
}

/// The window's ask for the refined masks' picture is answered for the
/// develop it names; with no picture open, with none.
#[test]
fn an_ask_for_the_refined_masks_picture_is_answered() {
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::MaskPicture {
            generation,
            picture,
        } = o
        {
            tx.lock()
                .unwrap()
                .send((generation, picture.is_some()))
                .unwrap();
        }
    });
    worker.send(Job::MaskPicture {
        edit: Box::new(refining(0.0)),
        generation: 7,
    });
    let answer = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("answered");
    assert_eq!(answer, (7, false));
    assert!(worker.stop());
}

/// The edit's pictures of a frame are refined as its export is: a
/// refined mask's reduced develop carries the picture beside its
/// guide, and the picture made from it is another than the same edit's
/// with the mask unrefined.
#[test]
fn the_edits_pictures_are_refined() {
    use greycard_edit::mask::Component;
    let dir = crate::testing::scratch_dir("refined-pictures");
    let path = dir.join("frame.dng");
    crate::testing::linear_dng(&path, 96, 64, "Canon", "Canon EOS R5");
    let mut look = greycard_edit::Look::default();
    look.light.exposure = 1.5;
    let mut edit = Edit::default();
    edit.adjustments.push(greycard_edit::Adjustment {
        id: 4,
        mask: greycard_edit::mask::Mask {
            components: vec![Component {
                shape: Shape::Linear {
                    from: [0.51, 0.3],
                    to: [0.5, 0.3],
                },
                ..Component::default()
            }],
            refine: 0.4,
            ..Default::default()
        },
        look,
        ..greycard_edit::Adjustment::default()
    });
    let picture = |edit: &Edit| {
        let developed = crate::reduced::develop(&path, edit, 0, None).expect("reduced");
        let finish = crate::edited_pictures::Finish {
            edit: edit.clone(),
            turn: 0,
            source: (developed.image.width as u32, developed.image.height as u32),
            full: developed.full,
            rasters: Default::default(),
            clip_level: developed.clip_level,
            guide: Some(developed.guide.clone()),
            kind: developed.kind,
            white: developed.white.clone(),
            detail: developed.detail.clone(),
            camera: developed.camera,
        };
        (
            developed.guide.picture.is_some(),
            crate::edited_pictures::make(&developed.image, &finish)
                .unwrap()
                .big,
        )
    };
    let (with, refined) = picture(&edit);
    let mut plain = edit.clone();
    plain.adjustments[0].mask.refine = 0.0;
    let (without, unrefined) = picture(&plain);
    assert!(with && !without);
    assert!(refined != unrefined, "the refine changed nothing");
    crate::testing::remove_dir_retry(&dir);
}

/// With the GPU, the refined masks' picture is made of the patched
/// picture in memory, as on the CPU, with the sharpen on or off: nothing
/// is read back from the device, and it is the CPU develop's within the
/// base's own GPU ops' rounding.
#[test]
fn with_the_gpu_the_refined_masks_picture_is_the_patched_ones() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the refined masks' picture on the GPU");
        return;
    };
    let frame = Arc::new(aberrated_frame(320, 240));
    let deliver: Deliver = Arc::new(|_| {});
    let picture = |edit: &Edit, gpu: &mut Option<greycard_gpu::Context>| {
        let mut base = None;
        let (outcome, _) = develop_job(
            &Input::Raw(frame.clone()),
            edit,
            0,
            1,
            &mut base,
            &mut Ai::new(),
            None,
            None,
            gpu,
            &deliver,
        );
        let Outcome::Developed { guide, .. } = outcome else {
            panic!("developed")
        };
        let picture = guide.picture.clone().expect("a picture");
        let patched = base.as_ref().unwrap().patched_picture();
        assert!(*picture == greycard_render::refine::mask_picture(&patched));
        picture
    };
    let mut gpu = Some(ctx);
    for sharpen in [false, true] {
        let mut edit = refining(0.3);
        edit.adjustments[0].mask.components[0] = greycard_edit::mask::Component::default();
        edit.detail.clarity = 0.4;
        edit.sharpen.enabled = sharpen;
        let on_gpu = picture(&edit, &mut gpu);
        assert!(gpu.is_some(), "the GPU kept");
        let on_cpu = picture(&edit, &mut None);
        let worst = on_gpu
            .lab
            .iter()
            .zip(&on_cpu.lab)
            .flat_map(|(a, b)| (0..3).map(move |c| (a[c].to_f32() - b[c].to_f32()).abs()))
            .fold(0f32, f32::max);
        assert!(worst < 5e-3, "sharpen {sharpen}: {worst} apart");
    }
}

/// An edit with a mask over the whole frame whose Detail has `sharpen`,
/// the global sharpen on or off.
fn masked_sharpen(sharpen: f32, global: bool) -> Edit {
    let mut edit = Edit::default();
    edit.sharpen.enabled = global;
    let mut a = greycard_edit::Adjustment {
        id: 1,
        ..Default::default()
    };
    a.mask.components.push(greycard_edit::mask::Component {
        shape: greycard_edit::mask::Shape::Linear {
            from: [0.5, 10.0],
            to: [0.5, 11.0],
        },
        ..Default::default()
    });
    a.look.detail.sharpen = sharpen;
    edit.adjustments.push(a);
    edit
}

/// A mask's Sharpen through the export: the open frame's export and a
/// fresh walk's are the same pixels, with the global sharpen on and
/// off, and each is another picture than the mask without it; at -1
/// over a sharpened frame it is near the frame unsharpened, and at +1
/// over an unsharpened one near the frame sharpened.
#[test]
fn a_masks_sharpen_is_in_the_export() {
    use greycard_render::export::Pixels;
    let input = Input::Raw(Arc::new(aberrated_frame(200, 150)));
    let sixteen = |r: &greycard_render::export::Rendered| match &r.pixels {
        Pixels::Sixteen(p) => p.clone(),
        Pixels::Eight(_) => panic!("a TIFF is sixteen bits"),
    };
    let export = |what: &str, edit: &Edit| {
        let ((developed, _), (walked, _)) = develop_and_walk(what, &input, edit, 0);
        same_pixels(&developed, &walked);
        sixteen(&walked)
    };
    let worst = |a: &[u16], b: &[u16]| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max();
    let sharp = export("mask-sharpen-sharp", &masked_sharpen(0.0, true));
    let soft = export("mask-sharpen-soft", &masked_sharpen(0.0, false));
    assert_ne!(sharp, soft, "the sharpen did nothing");
    let off = export("mask-sharpen-off", &masked_sharpen(-1.0, true));
    assert_ne!(off, sharp);
    let on = export("mask-sharpen-on", &masked_sharpen(1.0, false));
    assert_ne!(on, soft);
    let (taken, put) = (worst(&off, &soft), worst(&on, &sharp));
    eprintln!("-1 over the sharpen: {taken:?} of 65535 off; +1 without: {put:?}");
    assert!(taken <= Some(10), "{taken:?}");
    assert!(put <= Some(10), "{put:?}");
}

/// A mask's Sharpen develops its delta on the GPU's path and the
/// CPU's alike: the planes come with it, the delta of one is the
/// other's to the planes' rounding, and with the global sharpen off
/// the picture is the unsharpened one, to the bit on each path.
#[test]
fn a_masks_sharpen_delta_is_made_on_both_paths() {
    let frame = Arc::new(aberrated_frame(400, 320));
    let deliver: Deliver = Arc::new(|_| {});
    let develop = |edit: &Edit, gpu: &mut Option<greycard_gpu::Context>| {
        let (outcome, _) = develop_job(
            &Input::Raw(frame.clone()),
            edit,
            0,
            1,
            &mut None,
            &mut Ai::new(),
            None,
            None,
            gpu,
            &deliver,
        );
        let Outcome::Developed { image, planes, .. } = outcome else {
            panic!("developed");
        };
        (image, planes)
    };
    let picture = |image: &Developed, gpu: &Option<greycard_gpu::Context>| -> Vec<f32> {
        match image {
            Developed::Texture(t) => read_viewport(gpu.as_ref().unwrap(), t).data,
            Developed::Halves(h) => h
                .pixels
                .chunks(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .map(|v| v.to_f32())
                .collect(),
        }
    };
    let mut paths = vec![None];
    match greycard_gpu::Context::own() {
        Ok(ctx) => paths.push(Some(ctx)),
        Err(_) => skipped("a mask's Sharpen on the GPU's path"),
    }
    let mut deltas = Vec::new();
    for mut gpu in paths {
        for global in [true, false] {
            // A share of the sharpen: positive when there is none.
            let amount = if global { -0.7 } else { 0.7 };
            let (image, planes) = develop(&masked_sharpen(amount, global), &mut gpu);
            let planes = planes.expect("the planes");
            assert!(planes.wants.sharpen && !planes.has_bands());
            let delta = planes.sharpen.clone().expect("the delta");
            assert!(delta.iter().any(|d| d.to_f32().abs() > 0.01));
            let (bare, none) = develop(&masked_sharpen(0.0, global), &mut gpu);
            assert!(none.is_none());
            // On the device's path the bare develop runs nothing there
            // and is made half floats on the CPU, the mask's in the
            // shader: the two may round a value a half float's step
            // apart.
            let (a, b) = (picture(&image, &gpu), picture(&bare, &gpu));
            let past = a
                .iter()
                .zip(&b)
                .filter(|(x, y)| (*x - *y).abs() > y.abs() * 1e-3 + 1e-7)
                .count();
            assert!(
                a.len() == b.len() && (if gpu.is_some() { past == 0 } else { a == b }),
                "global {global}, GPU {}: {past} values off the develop's without the mask",
                gpu.is_some()
            );
            deltas.push(delta);
        }
    }
    if deltas.len() == 4 {
        for (cpu, gpu) in [(&deltas[0], &deltas[2]), (&deltas[1], &deltas[3])] {
            let worst = cpu
                .iter()
                .zip(gpu.iter())
                .map(|(a, b)| (a.to_f32() - b.to_f32()).abs())
                .fold(0f32, f32::max);
            assert!(
                worst <= 1e-3,
                "the GPU's delta is {worst} stops off the CPU's"
            );
        }
    }
}

/// With the picture's sharpen off, a mask's negative Sharpen is no
/// share of a sharpen that never ran: no delta is made, and the export
/// is the unsharpened picture, to the bit.
#[test]
fn a_negative_masks_sharpen_without_the_global_is_nothing() {
    use greycard_render::export::Pixels;
    let input = Input::Raw(Arc::new(aberrated_frame(200, 150)));
    let sixteen = |r: &greycard_render::export::Rendered| match &r.pixels {
        Pixels::Sixteen(p) => p.clone(),
        Pixels::Eight(_) => panic!("a TIFF is sixteen bits"),
    };
    let negative = masked_sharpen(-1.0, false);
    assert!(!negative.detail_wants().any());
    let ((developed, _), (walked, _)) = develop_and_walk("mask-sharpen-neg", &input, &negative, 0);
    same_pixels(&developed, &walked);
    let ((_, _), (bare, _)) = develop_and_walk(
        "mask-sharpen-neg-bare",
        &input,
        &masked_sharpen(0.0, false),
        0,
    );
    assert!(sixteen(&walked) == sixteen(&bare), "the export moved");
    let deliver: Deliver = Arc::new(|_| {});
    let (outcome, _) = develop_job(
        &input,
        &negative,
        0,
        1,
        &mut None,
        &mut Ai::new(),
        None,
        None,
        &mut None,
        &deliver,
    );
    let Outcome::Developed { planes, .. } = outcome else {
        panic!("developed");
    };
    assert!(planes.is_none(), "no delta made");
}

/// The delta kept for the masks goes with the picture it was made of:
/// a develop after it that sharpens a changed picture without one
/// leaves no delta for the kept pictures to read.
#[test]
fn a_kept_sharpen_delta_does_not_outlive_its_picture() {
    let input = Input::Raw(Arc::new(aberrated_frame(200, 150)));
    let deliver: Deliver = Arc::new(|_| {});
    let mut base = None;
    let develop = |edit: &Edit, base: &mut Option<FrameKept>| {
        let (outcome, _) = develop_job(
            &input,
            edit,
            0,
            1,
            base,
            &mut Ai::new(),
            None,
            None,
            &mut None,
            &deliver,
        );
        assert!(matches!(outcome, Outcome::Developed { .. }));
    };
    let asking = masked_sharpen(-0.7, true);
    develop(&asking, &mut base);
    let planes = base.as_ref().unwrap().planes_for(&asking).expect("kept");
    assert!(planes.sharpen.is_some());
    // The global Texture moved and the mask's Sharpen gone: the
    // sharpen runs again on another picture, with no delta.
    let mut other = masked_sharpen(0.0, true);
    other.detail.texture = 0.4;
    develop(&other, &mut base);
    assert!(
        base.as_ref().unwrap().planes_for(&asking).is_none(),
        "the old picture's delta is kept"
    );
}

/// A flat picture with three Fill patches apart, made with no model.
fn three_fills() -> (Input, Edit, Ai) {
    use greycard_edit::retouch::{Method, Patch};
    let mut image = WorkingImage::new(256, 256);
    image.data.fill(0.4);
    let fill = |id, u: f32| Patch {
        id,
        method: Method::Fill,
        points: vec![[u, u]],
        radius: 0.04,
        ..Default::default()
    };
    let edit = Edit {
        retouch: Retouch {
            enabled: true,
            patches: vec![fill(1, 0.2), fill(2, 0.5), fill(3, 0.8)],
        },
        ..Default::default()
    };
    let mut ai = Ai::for_test(greycard_ai::Store::at("no-store"), Vec::new());
    ai.fill_by_test = Some(0.9);
    (picture_input(&Arc::new(image)), edit, ai)
}

/// The value at `(u, u)` of a developed picture.
fn at(image: &WorkingImage, u: f32) -> f32 {
    let x = (u * image.width as f32) as usize;
    let y = (u * image.height as f32) as usize;
    image.data[(y * image.width + x) * 3]
}

/// The window's develop delivers its picture before its fills, with
/// each left as it was; each round after makes the next, says where
/// the develop's count is, and delivers the picture with it in; the
/// last leaves none, and a develop after keeps them all.
#[test]
fn a_develop_shows_its_picture_and_then_makes_its_fills_one_a_round() {
    let (input, edit, mut ai) = three_fills();
    let said = Arc::new(std::sync::Mutex::new(Vec::new()));
    let deliver: Deliver = {
        let said = said.clone();
        Arc::new(move |o| {
            if let Outcome::Filling { name, made, of, .. } = o {
                said.lock().unwrap().push((name, made, of));
            }
        })
    };
    let mut base = None;
    let mut develop = |round: &mut Round, base: &mut Option<FrameKept>| {
        let (outcome, image) = develop_round(
            &input,
            &edit,
            0,
            1,
            base,
            &mut ai,
            (None, None),
            &mut None,
            &deliver,
            round,
            &crate::walk::NoHold,
        );
        let Outcome::Developed { fills, .. } = outcome else {
            panic!("developed");
        };
        (fills, image.expect("on the CPU"))
    };
    let mut round = Round::first();
    let (fills, image) = develop(&mut round, &mut base);
    assert!(fills.made.is_empty());
    assert_eq!(fills.pending, [1, 2, 3]);
    assert!(base.as_ref().unwrap().filling);
    for u in [0.2, 0.5, 0.8] {
        assert_eq!(at(&image, u), 0.4, "left as it was at {u}");
    }
    assert!(said.lock().unwrap().is_empty(), "no model ran");
    assert_eq!(round.ahead, (3, 0));
    for (k, left) in [(1, vec![2, 3]), (2, vec![3]), (3, vec![])] {
        round = Round::next(round.ahead);
        let (fills, image) = develop(&mut round, &mut base);
        assert_eq!(fills.made, [format!("Fill {k}")]);
        assert_eq!(fills.pending, left);
        assert_eq!(round.ahead, (3, k));
        for (i, u) in [0.2f32, 0.5, 0.8].into_iter().enumerate() {
            let want = if i < k { 0.9 } else { 0.4 };
            assert!((at(&image, u) - want).abs() < 5e-3, "round {k} at {u}");
        }
    }
    assert!(!base.as_ref().unwrap().filling);
    assert_eq!(
        *said.lock().unwrap(),
        [
            ("Fill 1".to_string(), 0, 3),
            ("Fill 2".to_string(), 1, 3),
            ("Fill 3".to_string(), 2, 3)
        ]
    );
    let mut again = Round::first();
    let (fills, _) = develop(&mut again, &mut base);
    assert!(fills.pending.is_empty() && fills.made.is_empty());
}

/// A crop that changes what a Fill's window sees has the fill made
/// again, and the fill made under the crop before stands in until the
/// new one lands: the develop the crop asks for shows the old fill,
/// still to make, never what the patch took out; the round after shows
/// the new one. A crop's fill that fails has no stand-in.
#[test]
fn a_fill_remade_for_a_crop_shows_the_old_one_until_it_lands() {
    use greycard_edit::retouch::{Method, Patch};
    let mut image = WorkingImage::new(256, 256);
    image.data.fill(0.4);
    let input = picture_input(&Arc::new(image));
    let mut edit = Edit {
        retouch: Retouch {
            enabled: true,
            patches: vec![Patch {
                id: 1,
                method: Method::Fill,
                points: vec![[0.93, 0.5]],
                radius: 0.04,
                ..Default::default()
            }],
        },
        ..Default::default()
    };
    let mut ai = Ai::for_test(greycard_ai::Store::at("no-store"), Vec::new());
    ai.fill_by_test = Some(0.9);
    let deliver: Deliver = Arc::new(|_| {});
    let mut base = None;
    let develop = |edit: &Edit, ai: &mut Ai, round: &mut Round, base: &mut Option<FrameKept>| {
        let (outcome, image) = develop_round(
            &input,
            edit,
            0,
            1,
            base,
            ai,
            (None, None),
            &mut None,
            &deliver,
            round,
            &crate::walk::NoHold,
        );
        let Outcome::Developed { fills, .. } = outcome else {
            panic!("developed");
        };
        let image = image.expect("on the CPU");
        // The fill's middle: the crop keeps the left edge.
        let v = image.data[(128 * image.width + 238) * 3];
        (fills.pending, v)
    };
    let mut round = Round::first();
    assert_eq!(
        develop(&edit, &mut ai, &mut round, &mut base),
        (vec![1], 0.4)
    );
    round = Round::next(round.ahead);
    let (left, v) = develop(&edit, &mut ai, &mut round, &mut base);
    assert!(left.is_empty() && (v - 0.9).abs() < 5e-3, "made: {v}");

    // Cropped over the fill's window, its edge hidden.
    edit.geometry.crop = Some(greycard_edit::geometry::Crop::of([0.0, 0.0, 0.97, 1.0]));
    ai.fill_by_test = Some(0.7);
    let mut round = Round::first();
    let (left, v) = develop(&edit, &mut ai, &mut round, &mut base);
    assert_eq!(left, [1], "made again");
    assert!((v - 0.9).abs() < 5e-3, "the old fill stands in, not {v}");
    round = Round::next(round.ahead);
    let (left, v) = develop(&edit, &mut ai, &mut round, &mut base);
    assert!(
        left.is_empty() && (v - 0.7).abs() < 5e-3,
        "the new fill: {v}"
    );

    // Another crop whose fill fails: the patch as it was.
    edit.geometry.crop = Some(greycard_edit::geometry::Crop::of([0.0, 0.0, 0.95, 1.0]));
    ai.fill_by_test = Some(-1.0);
    let mut round = Round::first();
    develop(&edit, &mut ai, &mut round, &mut base);
    round = Round::next(round.ahead);
    let (_, v) = develop(&edit, &mut ai, &mut round, &mut base);
    assert_eq!(v, 0.4, "a failed fill has no stand-in");
}

/// A removal remade for a crop on a card that refuses it each round:
/// its stand-in shows while it is still to come, and once its rounds
/// end stuck the next develop shows the patch as it was, not the old
/// crop's fill kept in a patched picture.
#[test]
fn a_refused_fill_ends_on_the_base_not_its_stand_in() {
    use super::engine::FillsDue;
    use greycard_edit::retouch::{FillChoice, Method, Patch};
    const GIB: u64 = 1 << 30;
    let mut image = WorkingImage::new(256, 256);
    image.data.fill(0.4);
    let input = picture_input(&Arc::new(image));
    let mut patch = Patch {
        id: 1,
        method: Method::Fill,
        points: vec![[0.93, 0.5]],
        radius: 0.04,
        remove: true,
        ..Default::default()
    };
    patch.set_fill_choice(FillChoice::Best);
    let mut edit = Edit {
        retouch: Retouch {
            enabled: true,
            patches: vec![patch],
        },
        ..Default::default()
    };
    let mut ai = Ai::for_test(greycard_ai::Store::at("no-store"), Vec::new());
    ai.fill_by_test = Some(0.9);
    ai.card_by_test = Some(Some(16 * GIB));
    ai.free_memory_by_test = Some(16 * GIB);
    ai.set_providers_by_test(vec![
        greycard_ai::Provider::WebGpu,
        greycard_ai::Provider::Cpu,
    ]);
    let deliver: Deliver = Arc::new(|_| {});
    let mut base = None;
    let develop = |edit: &Edit, ai: &mut Ai, round: &mut Round, base: &mut Option<FrameKept>| {
        let (outcome, image) = develop_round(
            &input,
            edit,
            0,
            1,
            base,
            ai,
            (None, None),
            &mut None,
            &deliver,
            round,
            &crate::walk::NoHold,
        );
        let image = image.expect("on the CPU");
        let v = image.data[(128 * image.width + 238) * 3];
        (outcome, v)
    };
    let pending = |o: &Outcome| match o {
        Outcome::Developed { fills, .. } => fills.pending.clone(),
        _ => panic!("developed"),
    };
    let mut round = Round::first();
    develop(&edit, &mut ai, &mut round, &mut base);
    round = Round::next(round.ahead);
    let (_, v) = develop(&edit, &mut ai, &mut round, &mut base);
    assert!((v - 0.9).abs() < 5e-3, "made: {v}");

    // Cropped over its window, on a card with too little free for it.
    edit.geometry.crop = Some(greycard_edit::geometry::Crop::of([0.0, 0.0, 0.97, 1.0]));
    ai.free_memory_by_test = Some(GIB / 4);
    let mut round = Round::first();
    let (outcome, v) = develop(&edit, &mut ai, &mut round, &mut base);
    assert_eq!(pending(&outcome), [1]);
    assert!((v - 0.9).abs() < 5e-3, "the stand-in: {v}");
    // Each round after refuses it, until the rounds end stuck.
    let mut left = Some(1);
    for _ in 0..4 {
        round = Round::next(round.ahead);
        let (mut outcome, _) = develop(&edit, &mut ai, &mut round, &mut base);
        let due = FillsDue::after(&mut outcome, &edit, 0, round, left, base.as_mut());
        match due {
            Some(_) => left = Some(pending(&outcome).len()),
            None => {
                left = None;
                break;
            }
        }
    }
    assert!(left.is_none(), "the rounds ended");
    // A slider past the retouch: the patch as it was.
    edit.light.exposure = 0.0001;
    let (_, v) = develop(&edit, &mut ai, &mut Round::first(), &mut base);
    assert!((v - 0.4).abs() < 5e-3, "the base, not the stand-in: {v}");
}

/// A develop that starts past its retouch, from a point that left fills
/// for later, says them still to make: a sharpen moved between two
/// rounds does not give the fills up.
#[test]
fn a_develop_past_its_retouch_still_has_its_fills_to_make() {
    let (input, edit, mut ai) = three_fills();
    let deliver: Deliver = Arc::new(|_| {});
    let mut base = None;
    let mut develop = |edit: &Edit, round: &mut Round, base: &mut Option<FrameKept>| {
        let (outcome, _) = develop_round(
            &input,
            edit,
            0,
            1,
            base,
            &mut ai,
            (None, None),
            &mut None,
            &deliver,
            round,
            &crate::walk::NoHold,
        );
        let Outcome::Developed { fills, .. } = outcome else {
            panic!("developed");
        };
        fills
    };
    develop(&edit, &mut Round::first(), &mut base);
    let mut round = Round::next((3, 0));
    develop(&edit, &mut round, &mut base);
    let mut sharper = edit.clone();
    sharper.sharpen.iterations += 3;
    let fills = develop(&sharper, &mut Round::first(), &mut base);
    assert!(fills.made.is_empty());
    assert_eq!(fills.pending, [2, 3]);
}

/// The worker delivers an open's picture with its fills still to make,
/// then each fill as a develop of the same generation; a develop asked
/// for while they are being made takes the rest over, so no fill is
/// made for the develop it replaced after it is asked.
#[test]
fn the_worker_makes_a_develops_fills_after_it_and_a_new_develop_takes_them_over() {
    let (dir, paths) = dngs("fill-rounds", &["fill-rounds.dng"], 128, 96);
    let (_, edit, _) = three_fills();
    // What the worker said, in order: each develop's generation and
    // the fills it left, and each fill started, by generation.
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    // Held by the first develop with fills left until the test has
    // asked for the next develop.
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let go_rx = Mutex::new(Some(go_rx));
    let worker = Worker::new(move |o| match o {
        Outcome::Developed {
            generation, fills, ..
        } => {
            let first = generation == 1 && fills.pending.len() == 3;
            tx.lock()
                .unwrap()
                .send(format!("developed {generation}: {:?}", fills.pending))
                .unwrap();
            if first && let Some(go) = go_rx.lock().unwrap().take() {
                go.recv().unwrap();
            }
        }
        Outcome::Filling {
            generation, name, ..
        } => tx
            .lock()
            .unwrap()
            .send(format!("filling {generation}: {name}"))
            .unwrap(),
        Outcome::Failed { message, .. } => panic!("{message}"),
        _ => {}
    });
    worker.fill_by_test(0.9);
    worker.send(Job::Open {
        path: paths[0].clone(),
        version: None,
        edit: edit.clone(),
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let wait = std::time::Duration::from_secs(60);
    let next = || rx.recv_timeout(wait).expect("the worker says");
    assert_eq!(next(), "developed 1: [1, 2, 3]");
    worker.send(Job::Develop {
        edit: edit.clone(),
        generation: 2,
        turn: 0,
        of: None,
    });
    go_tx.send(()).unwrap();
    let said: Vec<String> = (0..7).map(|_| next()).collect();
    assert_eq!(
        said,
        [
            "developed 2: [1, 2, 3]",
            "filling 2: Fill 1",
            "developed 2: [2, 3]",
            "filling 2: Fill 2",
            "developed 2: [3]",
            "filling 2: Fill 3",
            "developed 2: []",
        ]
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A worker on a raw with [`three_fills`]'s edit, its fills made with
/// no model of `value`: what it says, in order (each develop's
/// generation and the fills it left, each fill started, each mask
/// answered), and a gate the first develop with fills left waits on
/// before it lets the worker go on.
fn fill_worker(
    what: &str,
    value: f32,
) -> (
    Worker,
    PathBuf,
    PathBuf,
    std::sync::mpsc::Receiver<String>,
    std::sync::mpsc::Sender<()>,
) {
    fill_worker_at(what, value, (128, 96))
}

/// [`fill_worker`] on a frame of `size`.
fn fill_worker_at(
    what: &str,
    value: f32,
    size: (usize, usize),
) -> (
    Worker,
    PathBuf,
    PathBuf,
    std::sync::mpsc::Receiver<String>,
    std::sync::mpsc::Sender<()>,
) {
    let (worker, dir, path, rx, go, _) = fill_worker_noting(what, value, size);
    (worker, dir, path, rx, go)
}

/// What each develop of [`fill_worker_noting`] said refused: its
/// generation, the fills failed, and the removals short of the card
/// with their need, bytes.
type Refusals = Arc<Mutex<Vec<(u64, Vec<String>, Vec<(String, u64)>)>>>;

/// [`fill_worker_at`], each develop that says a fill refused noted.
#[allow(clippy::type_complexity)]
fn fill_worker_noting(
    what: &str,
    value: f32,
    (width, height): (usize, usize),
) -> (
    Worker,
    PathBuf,
    PathBuf,
    std::sync::mpsc::Receiver<String>,
    std::sync::mpsc::Sender<()>,
    Refusals,
) {
    let refusals: Refusals = Arc::default();
    let noted = refusals.clone();
    let (dir, paths) = dngs(what, &[&format!("{what}.dng")], width, height);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let go_rx = Mutex::new(Some(go_rx));
    let worker = Worker::new(move |o| match o {
        Outcome::Developed {
            generation, fills, ..
        } => {
            let first = fills.pending.len() == 3;
            if !fills.failed.is_empty() || !fills.short.is_empty() {
                noted.lock().unwrap().push((
                    generation,
                    fills.failed.iter().map(|(n, _)| n.clone()).collect(),
                    fills
                        .short
                        .iter()
                        .map(|(n, need, _)| (n.clone(), *need))
                        .collect(),
                ));
            }
            tx.lock()
                .unwrap()
                .send(format!("developed {generation}: {:?}", fills.pending))
                .unwrap();
            if first && let Some(go) = go_rx.lock().unwrap().take() {
                go.recv().unwrap();
            }
        }
        Outcome::Filling {
            generation, name, ..
        } => tx
            .lock()
            .unwrap()
            .send(format!("filling {generation}: {name}"))
            .unwrap(),
        Outcome::Mask { .. } | Outcome::MaskFailed { .. } => {
            tx.lock().unwrap().send("mask".into()).unwrap()
        }
        Outcome::Failed { message, .. } => panic!("{message}"),
        _ => {}
    });
    worker.fill_by_test(value);
    (worker, dir, paths[0].clone(), rx, go_tx, refusals)
}

fn open_with_fills(path: &std::path::Path) -> Job {
    Job::Open {
        path: path.to_path_buf(),
        version: None,
        edit: three_fills().1,
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    }
}

/// A fill whose model will not load fails each round without being
/// remembered: the rounds end once a round leaves no fewer to make,
/// and the last develop says none to come.
#[test]
fn rounds_whose_fill_fails_each_time_end() {
    let (worker, dir, path, rx, go) = fill_worker("fill-rounds-fail", -1.0);
    go.send(()).unwrap();
    worker.send(open_with_fills(&path));
    let wait = std::time::Duration::from_secs(60);
    let said: Vec<String> = (0..5)
        .map(|_| rx.recv_timeout(wait).expect("the worker says"))
        .collect();
    assert_eq!(
        said,
        [
            "developed 1: [1, 2, 3]",
            "filling 1: Fill 1",
            "developed 1: [2, 3]",
            "filling 1: Fill 1",
            "developed 1: []",
        ]
    );
    // The queue is over: a mask, which waits behind it, is answered.
    worker.send(Job::Mask {
        key: (1, 0),
        shape: Shape::Linear {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
        },
        model: None,
    });
    assert_eq!(rx.recv_timeout(wait).expect("the mask"), "mask");
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// Leaving the editor pauses the open frame's fills: a mask asked for
/// after it, which waits behind the fills, is answered with no fill
/// made; the frame shown again, the rest are made.
#[test]
fn leaving_the_editor_pauses_the_fills() {
    let (worker, dir, path, rx, go) = fill_worker("fill-rounds-leave", 0.9);
    worker.send(open_with_fills(&path));
    let wait = std::time::Duration::from_secs(60);
    assert_eq!(
        rx.recv_timeout(wait).expect("the open"),
        "developed 1: [1, 2, 3]"
    );
    worker.edited().set_open(None);
    worker.send(Job::Mask {
        key: (1, 0),
        shape: Shape::Linear {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
        },
        model: None,
    });
    go.send(()).unwrap();
    assert_eq!(rx.recv_timeout(wait).expect("the mask"), "mask");
    worker.set_open_frame(Some(Frame {
        path: path.clone(),
        version: None,
        name: None,
    }));
    let said: Vec<String> = (0..6)
        .map(|_| rx.recv_timeout(wait).expect("the worker says"))
        .collect();
    assert_eq!(
        said,
        [
            "filling 1: Fill 1",
            "developed 1: [2, 3]",
            "filling 1: Fill 2",
            "developed 1: [3]",
            "filling 1: Fill 3",
            "developed 1: []",
        ]
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// The worker gives a card short of room back once a queue of fills
/// ends, once when a queue is given up for another job, and after an
/// export frame whose fills ran a model; a model that ran nothing gives
/// nothing back, and a card with room keeps the sessions.
#[test]
fn the_worker_gives_the_card_back_when_its_fills_are_done() {
    const GIB: u64 = 1 << 30;
    // With room, the sessions are kept at the queue's end.
    let (worker, dir, path, rx, go) = fill_worker("give-back-room", 0.9);
    worker.free_by_test(12 * GIB, GIB, 0);
    go.send(()).unwrap();
    worker.send(open_with_fills(&path));
    let wait = std::time::Duration::from_secs(60);
    let next = || rx.recv_timeout(wait).expect("the worker says");
    while next() != "developed 1: []" {}
    worker.send(Job::Mask {
        key: (1, 0),
        shape: Shape::Linear {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
        },
        model: None,
    });
    assert_eq!(next(), "mask");
    assert_eq!(worker.gave_back_by_test(), 0);
    worker.stop();
    crate::testing::remove_dir_retry(&dir);

    // Short of room, at the queue's end.
    let (worker, dir, path, rx, go) = fill_worker("give-back-end", 0.9);
    worker.free_by_test(3 * GIB, GIB, 0);
    go.send(()).unwrap();
    worker.send(open_with_fills(&path));
    let wait = std::time::Duration::from_secs(60);
    let next = || rx.recv_timeout(wait).expect("the worker says");
    while next() != "developed 1: []" {}
    // Delivered before the give-back: a mask behind it is answered
    // after.
    worker.send(Job::Mask {
        key: (1, 0),
        shape: Shape::Linear {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
        },
        model: None,
    });
    assert_eq!(next(), "mask");
    assert_eq!(worker.gave_back_by_test(), 1);
    worker.stop();
    crate::testing::remove_dir_retry(&dir);

    // Given up for another develop: once, before it.
    let (worker, dir, path, rx, go) = fill_worker("give-back-change", 0.9);
    worker.free_by_test(3 * GIB, GIB, 0);
    go.send(()).unwrap();
    worker.send(open_with_fills(&path));
    let next = || rx.recv_timeout(wait).expect("the worker says");
    while next() != "developed 1: [2, 3]" {}
    worker.send(Job::Develop {
        edit: three_fills().1,
        generation: 2,
        turn: 0,
        of: None,
    });
    while !next().starts_with("developed 2") {}
    assert_eq!(worker.gave_back_by_test(), 1);
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// An export whose fill the card stays short of fails its item, the
/// card asked its tries; with room it is written. The rest of the walk
/// does not wait again.
#[test]
fn an_export_fails_its_item_on_a_card_short_of_a_fill() {
    let (input, mut edit, _) = three_fills();
    for p in &mut edit.retouch.patches {
        p.remove = true;
    }
    let mut ai = Ai::for_test(
        greycard_ai::Store::at("no-store"),
        vec![greycard_ai::Provider::WebGpu, greycard_ai::Provider::Cpu],
    );
    ai.fill_by_test = Some(0.9);
    ai.free_memory_by_test = Some(1 << 30);
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = asked.clone();
    ai.set_patience(Some(crate::ai::Patience {
        tries: 3,
        wait: std::time::Duration::ZERO,
        canceled: Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            false
        }),
    }));
    let deliver: Deliver = Arc::new(|_| {});
    let settings = greycard_render::export::Settings::default();
    let why = super::export::walk_export(
        input.clone(),
        &edit,
        0,
        &mut WorkerModels::new(&mut ai, None, &deliver, 0),
        None,
        &mut None,
        &settings,
    )
    .err()
    .expect("refused");
    // Said as the window says it; the reason is the log's.
    assert_eq!(why, "Needs 6.3 GiB VRAM, 1.0 available. Use Patch.");
    // Two waits for the first fill, none for the two after it.
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 2);
    ai.free_memory_by_test = Some(16 << 30);
    ai.set_patience(Some(crate::ai::Patience {
        wait: std::time::Duration::ZERO,
        ..crate::ai::Patience::of(3)
    }));
    super::export::walk_export(
        input,
        &edit,
        0,
        &mut WorkerModels::new(&mut ai, None, &deliver, 0),
        None,
        &mut None,
        &settings,
    )
    .expect("written");
}

/// A set met short is remembered by the set itself: not by another
/// set, and not once it is done or canceled.
#[test]
fn a_set_met_short_is_remembered_by_itself_alone() {
    let set = || {
        Arc::new(crate::queue::Set::new(
            2,
            None,
            greycard_render::export::Settings::default(),
            greycard_render::export::OnExists::default(),
        ))
    };
    let mut sets = super::engine::ShortSets::default();
    let (a, b) = (set(), set());
    sets.insert(&a);
    assert!(sets.contains(&a) && !sets.contains(&b));
    sets.remove(&a);
    assert!(!sets.contains(&a));
    sets.insert(&b);
    b.cancel();
    sets.insert(&a);
    assert!(!sets.contains(&b), "a canceled set is forgotten");
    drop(a);
    let c = set();
    assert!(!sets.contains(&c), "a new set is not an old one");
}

/// What a worker says of `edit`'s fills on a frame of `size`, up to its
/// answer to a mask asked after them, with the card as
/// [`Worker::free_by_test`] has it; and how often it gave the card back.
/// The mask waits behind the fills in the worker's pick, so it is
/// answered once they are done.
fn fills_said(
    what: &str,
    size: (usize, usize),
    edit: Edit,
    card: (u64, u64, u64),
) -> (Vec<String>, usize) {
    let (said, gave) = fills_said_then(what, size, edit, card, None);
    (said, gave[0])
}

/// [`fills_said`], and then what it says of `then` up to a mask asked
/// after it; how often it gave the card back by each.
fn fills_said_then(
    what: &str,
    size: (usize, usize),
    edit: Edit,
    card: (u64, u64, u64),
    then: Option<Job>,
) -> (Vec<String>, Vec<usize>) {
    let (said, gave, _, _) = fills_said_at(what, size, edit, card, None, then);
    (said, gave)
}

/// [`fills_said_then`] with a UNet held for a queue as `unet` has it
/// (`Worker::unet_by_test`), and each fill begun with the side it ran
/// at.
fn fills_said_at(
    what: &str,
    size: (usize, usize),
    edit: Edit,
    (free, freed, elsewhere): (u64, u64, u64),
    unet: Option<(usize, u64)>,
    then: Option<Job>,
) -> FillsSaid {
    let (worker, dir, path, rx, go, refusals) = fill_worker_noting(what, 0.9, size);
    worker.free_by_test(free, freed, elsewhere);
    if let Some((side, bytes)) = unet {
        worker.unet_by_test(side, bytes);
    }
    go.send(()).unwrap();
    worker.send(Job::Open {
        path: path.to_path_buf(),
        version: None,
        edit,
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let mask = || Job::Mask {
        key: (1, 0),
        shape: Shape::Linear {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
        },
        model: None,
    };
    worker.send(mask());
    let mut said = Vec::new();
    let to_mask = |said: &mut Vec<String>| loop {
        let s = rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the worker says");
        if s == "mask" {
            break;
        }
        said.push(s);
    };
    to_mask(&mut said);
    let mut gave = vec![worker.gave_back_by_test()];
    if let Some(job) = then {
        said.push("mask".into());
        worker.send(job);
        worker.send(mask());
        to_mask(&mut said);
        gave.push(worker.gave_back_by_test());
    }
    let ran_at = worker.ran_at_by_test();
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
    let refused = refusals.lock().unwrap().clone();
    (said, gave, ran_at, refused)
}

/// What [`fills_said_at`] saw: the worker's words, its give-backs by
/// job, each fill begun with its side, and each develop's refusals.
type FillsSaid = (
    Vec<String>,
    Vec<usize>,
    Vec<(String, usize)>,
    Vec<(u64, Vec<String>, Vec<(String, u64)>)>,
);

/// [`three_fills`]' edit, each a removal.
fn three_removes() -> Edit {
    let mut edit = three_fills().1;
    for p in &mut edit.retouch.patches {
        p.remove = true;
    }
    edit
}

/// Removals marked Fast, on a card short of them now (one under the
/// remove's need, say), are left as they were, as on any short card,
/// and each leaves the need and the card's free memory for the
/// window's word on it.
#[test]
fn a_fast_removal_on_a_short_card_fails_with_its_figures() {
    const GIB: u64 = 1 << 30;
    let (dir, paths) = dngs("fast-short", &["fast-short.dng"], 128, 96);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed { fills, .. } = o {
            let failed: Vec<String> = fills.failed.iter().map(|(n, _)| n.clone()).collect();
            tx.lock()
                .unwrap()
                .send((fills.pending.clone(), failed, fills.short.clone()))
                .unwrap();
        }
    });
    worker.models_by_test(greycard_ai::Store::at(dir.join("models")));
    worker.fill_by_test(0.9);
    worker.free_by_test(4 * GIB, 0, 0);
    let mut edit = three_removes();
    for p in &mut edit.retouch.patches {
        p.set_fill_choice(greycard_edit::retouch::FillChoice::Fast);
    }
    worker.send(Job::Open {
        path: paths[0].clone(),
        version: None,
        edit,
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let wait = std::time::Duration::from_secs(60);
    let (mut failed, mut short) = (Vec::new(), Vec::new());
    while failed.len() < 3 {
        let (pending, f, s) = rx.recv_timeout(wait).expect("a develop");
        failed.extend(f);
        short.extend(s);
        if pending.is_empty() && !failed.is_empty() {
            break;
        }
    }
    assert_eq!(failed.len(), 3, "{failed:?}");
    let names: Vec<String> = short.iter().map(|(n, ..)| n.clone()).collect();
    assert_eq!(names, failed);
    let need = crate::ai::remove_needs(512, false);
    assert!(
        short.iter().all(|&(_, n, f)| (n, f) == (need, 4 * GIB)),
        "{short:?}"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// On a card under the remove's need, a develop that moves a slider
/// past the retouch starts from the kept patched picture (the removals
/// are not asked again) and still says each one's need and the card;
/// a removal made Patch says nothing more.
#[test]
fn a_develop_past_the_retouch_still_says_the_card() {
    const GIB: u64 = 1 << 30;
    let (input, mut edit, mut ai) = three_fills();
    for p in &mut edit.retouch.patches {
        p.set_fill_choice(greycard_edit::retouch::FillChoice::Best);
    }
    ai.card_by_test = Some(Some(4 * GIB));
    let deliver: Deliver = Arc::new(|_| {});
    let mut base = None;
    let mut develop = |edit: &Edit, generation: u64| {
        let (outcome, _) = develop_job(
            &input, edit, 0, generation, &mut base, &mut ai, None, None, &mut None, &deliver,
        );
        let Outcome::Developed { fills, .. } = outcome else {
            panic!("developed");
        };
        fills
    };
    let need = crate::ai::remove_needs(512, false);
    let said = |names: &[&str]| -> Vec<(String, u64, u64)> {
        names
            .iter()
            .map(|n| (n.to_string(), need, 4 * GIB))
            .collect()
    };
    let first = develop(&edit, 1);
    assert_eq!(first.failed.len(), 3, "refused for the card");
    assert_eq!(first.short, said(&["Fill 1", "Fill 2", "Fill 3"]));
    // A slider past the retouch: from the kept point, nothing asked.
    edit.light.exposure = 0.5;
    let moved = develop(&edit, 2);
    assert!(moved.failed.is_empty(), "the retouch did not run again");
    assert!(moved.pending.is_empty());
    assert_eq!(moved.short, said(&["Fill 1", "Fill 2", "Fill 3"]));
    // Fill 2 made Patch: made, and no more said of it.
    edit.retouch.patches[1].set_fill_choice(greycard_edit::retouch::FillChoice::Patch);
    let patched = develop(&edit, 3);
    assert_eq!(patched.short, said(&["Fill 1", "Fill 3"]));
    assert!(
        patched.made.contains(&"Fill 2".to_string()),
        "{:?}",
        patched.made
    );
}

/// A frame on a card under the remove's need, left for another and
/// opened again, its patched picture taken back from the shelf: its
/// first develop and a slider past the retouch after it still say the
/// card, worked out afresh, though the retouch never runs again.
#[test]
fn a_frame_taken_back_from_the_shelf_still_says_the_card() {
    use super::engine::swap_base;
    use super::shelf::{Budget, Shelf};
    const GIB: u64 = 1 << 30;
    let frame = Arc::new(aberrated_frame(160, 120));
    let other = Arc::new(aberrated_frame(96, 64));
    let mut edit = three_removes();
    edit.retouch.patches.truncate(1);
    edit.retouch.patches[0].set_fill_choice(greycard_edit::retouch::FillChoice::Best);
    let mut ai = Ai::for_test(greycard_ai::Store::at("no-store"), Vec::new());
    ai.fill_by_test = Some(0.9);
    ai.card_by_test = Some(Some(4 * GIB));
    let deliver: Deliver = Arc::new(|_| {});
    let mut base = None;
    let develop =
        |frame: &Arc<RawFrame>, edit: &Edit, base: &mut Option<FrameKept>, ai: &mut Ai| {
            let (outcome, _) = develop_job(
                &Input::Raw(frame.clone()),
                edit,
                0,
                1,
                base,
                ai,
                None,
                None,
                &mut None,
                &deliver,
            );
            let Outcome::Developed { fills, .. } = outcome else {
                panic!("developed");
            };
            fills
        };
    let said = vec![(
        "Fill 1".to_string(),
        crate::ai::remove_needs(512, false),
        4 * GIB,
    )];
    ai.forget(Some("a.CR3".into()));
    let first = develop(&frame, &edit, &mut base, &mut ai);
    assert_eq!(first.failed.len(), 1);
    assert_eq!(first.short, said);
    // Away to B, and back to A from the shelf.
    let (a, b) = (shelf_key("a.CR3", &frame), shelf_key("b.CR3", &other));
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    swap_base(
        &mut base,
        &mut shelf,
        (Some(a.clone()), None),
        (&edit, 0, &|_, _| false),
        None,
        None,
    );
    ai.forget(Some("b.CR3".into()));
    develop(&other, &Edit::default(), &mut base, &mut ai);
    swap_base(
        &mut base,
        &mut shelf,
        (Some(b), Some((&a, &Input::Raw(frame.clone())))),
        (&edit, 0, &|_, _| false),
        None,
        None,
    );
    ai.forget(Some("a.CR3".into()));
    let again = develop(&frame, &edit, &mut base, &mut ai);
    assert!(again.failed.is_empty(), "served from the shelf");
    assert_eq!(again.short, said);
    let mut moved = edit.clone();
    moved.light.exposure = 0.4;
    let past = develop(&frame, &moved, &mut base, &mut ai);
    assert!(past.failed.is_empty(), "the retouch did not run again");
    assert_eq!(past.short, said);
}

/// The card's word follows the picture shown, worked out afresh: a
/// removal made under one white balance, refused for the card under
/// another, is said there; back under the first, its fill shown from
/// memory, nothing is said. With the card freed, the fill asked again
/// is not said beside its making; and a fill that fails for another
/// reason is said as that failure, not hidden behind the card.
#[test]
fn the_cards_word_follows_the_fill_shown() {
    const GIB: u64 = 1 << 30;
    let (dir, paths) = dngs("word-follows", &["word-follows.dng"], 128, 96);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed {
            generation, fills, ..
        } = o
        {
            let failed: Vec<String> = fills.failed.iter().map(|(n, _)| n.clone()).collect();
            tx.lock()
                .unwrap()
                .send((
                    generation,
                    fills.pending.clone(),
                    failed,
                    fills.made.clone(),
                    fills.short.clone(),
                ))
                .unwrap();
        }
    });
    worker.models_by_test(greycard_ai::Store::at(dir.join("models")));
    worker.fill_by_test(0.9);
    worker.free_by_test(16 * GIB, 0, 0);
    let mut a = three_removes();
    a.retouch.patches.truncate(1);
    let white = |kelvin: f64| {
        let mut e = a.clone();
        e.white_balance = greycard_edit::WhiteBalance::Custom {
            temperature: kelvin,
            tint: 0.0,
        };
        e
    };
    let (b, c) = (white(3500.0), white(7000.0));
    type Said = (
        u64,
        Vec<u64>,
        Vec<String>,
        Vec<String>,
        Vec<(String, u64, u64)>,
    );
    let wait = std::time::Duration::from_secs(60);
    let until = |generation: u64, done: &dyn Fn(&Said) -> bool| -> Vec<Said> {
        let mut all = Vec::new();
        loop {
            let s: Said = rx.recv_timeout(wait).expect("a develop");
            if s.0 != generation {
                continue;
            }
            let end = done(&s);
            all.push(s);
            if end {
                return all;
            }
        }
    };
    let develop = |edit: &Edit, generation: u64| Job::Develop {
        edit: edit.clone(),
        generation,
        turn: 0,
        of: None,
    };
    worker.send(Job::Open {
        path: paths[0].clone(),
        version: None,
        edit: a.clone(),
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let made = until(1, &|s| !s.3.is_empty());
    assert!(made.iter().all(|s| s.4.is_empty()));
    // Under B, the card short: refused and said.
    worker.free_by_test(2 * GIB, 0, 0);
    worker.send(develop(&b, 2));
    let refused = until(2, &|s| !s.2.is_empty());
    let need = crate::ai::remove_needs(512, false);
    assert_eq!(
        refused.last().unwrap().4,
        [("Fill 1".to_string(), need, 2 * GIB)]
    );
    // Back under A: its fill from memory, on screen, and nothing said.
    worker.send(develop(&a, 3));
    let back = until(3, &|_| true);
    assert!(back[0].2.is_empty() && back[0].1.is_empty(), "{back:?}");
    assert!(back[0].4.is_empty(), "no old word: {back:?}");
    // The card freed, under B again: asked again, made, never said.
    worker.free_by_test(16 * GIB, 0, 0);
    worker.send(develop(&b, 4));
    let again = until(4, &|s| !s.3.is_empty());
    assert!(again[0].1.contains(&1), "asked again: {again:?}");
    assert!(again.iter().all(|s| s.4.is_empty()), "{again:?}");
    // A model's failure is its own word.
    worker.fill_by_test(-1.0);
    worker.send(develop(&c, 5));
    let failed = until(5, &|s| !s.2.is_empty());
    let last = failed.last().unwrap();
    assert_eq!(last.2, ["Fill 1"]);
    assert!(last.4.is_empty(), "not hidden behind the card");
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A removal the card is short of now, developed again with nothing
/// changed, says the same need and free figure each time: the second
/// develop's refusal is the first one's, kept, not a model's failure.
#[test]
fn a_short_removal_says_its_figures_on_every_develop() {
    const GIB: u64 = 1 << 30;
    let (dir, paths) = dngs("short-twice", &["short-twice.dng"], 128, 96);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed {
            generation, fills, ..
        } = o
        {
            tx.lock()
                .unwrap()
                .send((generation, fills.failed.len(), fills.short.clone()))
                .unwrap();
        }
    });
    worker.models_by_test(greycard_ai::Store::at(dir.join("models")));
    worker.fill_by_test(0.9);
    worker.free_by_test(2 * GIB, 0, 0);
    let mut edit = three_removes();
    edit.retouch.patches.truncate(1);
    worker.send(Job::Open {
        path: paths[0].clone(),
        version: None,
        edit: edit.clone(),
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let wait = std::time::Duration::from_secs(60);
    let said = |generation: u64| loop {
        let (g, failed, short) = rx.recv_timeout(wait).expect("a develop");
        if g == generation && failed > 0 {
            break short;
        }
    };
    let first = said(1);
    let need = crate::ai::remove_needs(512, false);
    assert_eq!(first, [("Fill 1".to_string(), need, 2 * GIB)]);
    // The patch's opacity, applied after its fill: the retouch runs
    // again, its fill as it was.
    edit.retouch.patches[0].opacity = 0.5;
    worker.send(Job::Develop {
        edit,
        generation: 2,
        turn: 0,
        of: None,
    });
    assert_eq!(said(2), first, "the same sentence again");
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A set short of the card by what the engine's own working sets hold
/// waits for nothing: each frame lets go of the sets its walk made
/// again before any wait, and its removals are made.
#[test]
fn a_set_short_by_the_engines_own_sets_waits_for_nothing() {
    const GIB: u64 = 1 << 30;
    let (worker, dir, path, rx, go) = fill_worker_at("waits-none", 0.9, (128, 96));
    worker.models_by_test(greycard_ai::Store::at(dir.join("models")));
    worker.fill_by_test(0.9);
    worker.free_by_test(2 * GIB, 0, 0);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    worker.fills_release_by_test(5 * GIB, calls.clone());
    go.send(()).unwrap();
    worker.send(Job::Open {
        path: path.clone(),
        version: None,
        edit: Edit::default(),
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let wait = std::time::Duration::from_secs(60);
    assert_eq!(rx.recv_timeout(wait).expect("the open"), "developed 1: []");
    let set = Arc::new(crate::queue::Set::new(
        3,
        None,
        greycard_render::export::Settings::default(),
        greycard_render::export::OnExists::Overwrite,
    ));
    let frame = |i: u32| {
        let mut edit = three_removes();
        for p in &mut edit.retouch.patches {
            p.seed = i + 1;
        }
        crate::queue::Frame {
            source: path.clone(),
            edit,
            version: None,
            turn: 0,
            time_shift: 0,
            seed_blend: false,
            out: dir.join(format!("waits-none-{i}.jpg")),
            moved_from: None,
        }
    };
    worker.send(Job::ExportSet {
        set: set.clone(),
        frames: vec![frame(0), frame(1), frame(2)],
    });
    let started = Instant::now();
    while set.tally().exported + set.tally().failed.len() < 3 {
        assert!(started.elapsed() < wait, "the set ends");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(set.tally().exported, 3, "{:?}", set.tally().failed);
    // The worker notes a frame's waits after the tally counts it.
    while worker.waits_by_test().len() < 3 {
        assert!(started.elapsed() < wait, "the last frame's waits noted");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(worker.waits_by_test(), [0, 0, 0]);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::Relaxed),
        3,
        "let go once a frame"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A set short of the card waits once: its first frame waits its tries
/// at its first removal, the walk's later asks not waiting, and fails;
/// the set's later frames ask once and are left out at once,
/// waiting for nothing.
#[test]
fn a_set_short_of_the_card_waits_once() {
    const GIB: u64 = 1 << 30;
    let (worker, dir, path, rx, go) = fill_worker_at("waits-once", 0.9, (128, 96));
    worker.models_by_test(greycard_ai::Store::at(dir.join("models")));
    worker.fill_by_test(0.9);
    worker.free_by_test(GIB, 0, 0);
    go.send(()).unwrap();
    worker.send(Job::Open {
        path: path.clone(),
        version: None,
        edit: Edit::default(),
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let wait = std::time::Duration::from_secs(60);
    assert_eq!(rx.recv_timeout(wait).expect("the open"), "developed 1: []");
    let set = Arc::new(crate::queue::Set::new(
        3,
        None,
        greycard_render::export::Settings::default(),
        greycard_render::export::OnExists::Overwrite,
    ));
    // Each frame its own fills, none kept: another seed a frame.
    let frame = |i: u32| {
        let mut edit = three_removes();
        for p in &mut edit.retouch.patches {
            p.seed = i + 1;
        }
        crate::queue::Frame {
            source: path.clone(),
            edit,
            version: None,
            turn: 0,
            time_shift: 0,
            seed_blend: false,
            out: dir.join(format!("waits-once-{i}.jpg")),
            moved_from: None,
        }
    };
    worker.send(Job::ExportSet {
        set: set.clone(),
        frames: vec![frame(0), frame(1), frame(2)],
    });
    let started = Instant::now();
    while set.tally().exported + set.tally().failed.len() < 3 {
        assert!(started.elapsed() < wait, "the set ends");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(set.tally().failed.len(), 3, "no frame's removals made");
    while worker.waits_by_test().len() < 3 {
        assert!(started.elapsed() < wait, "the last frame's waits noted");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let waits = worker.waits_by_test();
    let first = waits[0];
    // Nothing of the worker's to give back, so the frame is not tried
    // again: the first pass's waits alone.
    assert_eq!(
        first,
        crate::ai::PATIENT_TRIES as usize - 1,
        "the first ask's waits"
    );
    assert_eq!(
        waits,
        [first, first, first],
        "the later frames wait for nothing"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A fill the card is short of at its run, once its own models are
/// given back, gives the card back across the worker's models and is
/// asked for again, once: with the room it is made, and the fills
/// after it; without, the fills are not asked again. Counting them
/// gave nothing back.
#[test]
fn a_short_fill_is_asked_for_again_once_after_the_card_is_given_back() {
    const GIB: u64 = 1 << 30;
    // 1 GiB free, 1 more from the fills' own: a removal (6.3) is short
    // at its run, its place in the round going to the next, short too;
    // 8 more from the others' make the room, and the fills, asked for
    // again, are all made.
    let (said, gave) = fills_said(
        "short-again",
        (128, 96),
        three_removes(),
        (GIB, GIB, 8 * GIB),
    );
    assert_eq!(
        said,
        [
            "developed 1: [1, 2, 3]",
            "filling 1: Fill 1",
            "filling 1: Fill 2",
            "filling 1: Fill 3",
            "developed 1: []",
            "filling 1: Fill 1",
            "developed 1: [2, 3]",
            "filling 1: Fill 2",
            "developed 1: [3]",
            "filling 1: Fill 3",
            "developed 1: []",
        ]
    );
    assert_eq!(gave, 1);
    // 1 more from the others is still short: asked again once, and no
    // more after it.
    let (said, gave) = fills_said("short-bound", (128, 96), three_removes(), (GIB, GIB, GIB));
    assert_eq!(
        said,
        [
            "developed 1: [1, 2, 3]",
            "filling 1: Fill 1",
            "filling 1: Fill 2",
            "filling 1: Fill 3",
            "developed 1: []",
            "filling 1: Fill 1",
            "filling 1: Fill 2",
            "filling 1: Fill 3",
            "developed 1: []",
        ]
    );
    // Each refusal's round gave back across the models.
    assert_eq!(gave, 2);
}

/// The open frame's fill jobs have the engine let go of what a
/// develop makes again when a removal is short of the card, as its
/// mask jobs do: 5.9 GiB free and 0.6 let go, the three removals are
/// made, the release asked once, and it is not left set after the
/// job. With nothing let go they are refused as before.
#[test]
fn a_fill_job_lets_go_for_a_removal_and_clears_it_after() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    const GIB: u64 = 1 << 30;
    let run = |what: &str, bytes: u64| {
        let (worker, dir, path, rx, go) = fill_worker_at(what, 0.9, (128, 96));
        worker.free_by_test(59 * GIB / 10, 0, 0);
        let calls = Arc::new(AtomicUsize::new(0));
        worker.fills_release_by_test(bytes, calls.clone());
        go.send(()).unwrap();
        worker.send(Job::Open {
            path: path.to_path_buf(),
            version: None,
            edit: three_removes(),
            generation: 1,
            seed_blend: false,
            turn: 0,
            stance_known: false,
            learned: Vec::new(),
        });
        worker.send(Job::Mask {
            key: (1, 0),
            shape: Shape::Linear {
                from: [0.2, 0.5],
                to: [0.8, 0.5],
            },
            model: None,
        });
        let mut said = Vec::new();
        loop {
            let s = rx
                .recv_timeout(std::time::Duration::from_secs(60))
                .expect("the worker says");
            if s == "mask" {
                break;
            }
            said.push(s);
        }
        let left = worker.release_left_by_test();
        worker.stop();
        crate::testing::remove_dir_retry(&dir);
        (said, calls.load(Ordering::Relaxed), left)
    };
    let (said, calls, left) = run("fill-release", 6 * GIB / 10);
    assert_eq!(
        said,
        [
            "developed 1: [1, 2, 3]",
            "filling 1: Fill 1",
            "developed 1: [2, 3]",
            "filling 1: Fill 2",
            "developed 1: [3]",
            "filling 1: Fill 3",
            "developed 1: []",
        ]
    );
    assert_eq!(calls, 1, "let go once, the card's room after");
    assert!(!left, "the release is the fill job's alone");
    let (said, calls, left) = run("fill-release-none", 0);
    assert!(
        !said.iter().any(|s| s == "developed 1: [2, 3]"),
        "nothing made: {said:?}"
    );
    assert!(calls >= 1, "asked at each refusal");
    assert!(!left);
}

/// The three fills' names with the side each ran at, in the order they
/// were made: the second, at `big`, last, refused in its turn.
fn ran_at(big: usize) -> Vec<(String, usize)> {
    vec![
        ("Fill 1".into(), 512),
        ("Fill 3".into(), 512),
        ("Fill 2".into(), big),
    ]
}

/// A big fill among small ones, the card short of it at 1024 alone and
/// nothing to give back: refused at 1024 in its turn, as it always was,
/// the small ones made; asked again after the card was given back
/// (nothing went), it runs at 512, the last resort. No develop says it
/// refused: still to come after its refusal at 1024. A develop after
/// asks no fill round and gives nothing back.
#[test]
fn a_big_fill_the_card_is_short_of_at_1024_runs_at_512() {
    const GIB: u64 = 1 << 30;
    let mut edit = three_removes();
    // A window past 512 runs at 1024 (7.1 GiB); the others at 512 (6.3).
    edit.retouch.patches[1].radius = 0.25;
    let then = Job::Develop {
        edit: edit.clone(),
        generation: 2,
        turn: 0,
        of: None,
    };
    let (said, gave, ran, refused) = fills_said_at(
        "short-big",
        (1600, 1200),
        edit,
        (6 * GIB + GIB / 2, 0, 0),
        None,
        Some(then),
    );
    assert_eq!(
        said,
        [
            "developed 1: [1, 2, 3]",
            "filling 1: Fill 1",
            "developed 1: [2, 3]",
            "filling 1: Fill 2",
            "filling 1: Fill 3",
            "developed 1: [2]",
            "filling 1: Fill 2",
            "developed 1: []",
            "mask",
            "developed 2: []",
        ]
    );
    assert_eq!(ran, ran_at(512));
    assert_eq!(refused, [], "never said refused");
    assert_eq!(gave[0], gave[1], "nothing given back by the develop");
}

/// The card short of every fill and nothing to give back anywhere: the
/// small ones are said refused in the first round; the big one, refused
/// at 1024 there, is still to come, and is said refused only from the
/// round that asked it again, at 512, with 512's need.
#[test]
fn a_big_fill_short_of_both_is_said_refused_at_512_alone() {
    const GIB: u64 = 1 << 30;
    let mut edit = three_removes();
    edit.retouch.patches[1].radius = 0.25;
    let (said, _, ran, refused) = fills_said_at(
        "short-both",
        (1600, 1200),
        edit,
        (6 * GIB, 0, 0),
        None,
        None,
    );
    assert!(ran.is_empty(), "{ran:?}");
    let need = crate::ai::remove_needs(512, false);
    let small = |n: &str| (n.to_string(), need);
    let all = (
        1,
        vec!["Fill 1".into(), "Fill 2".into(), "Fill 3".into()],
        vec![small("Fill 1"), small("Fill 2"), small("Fill 3")],
    );
    // The round after it says the refusal again, as for any fill the
    // card stays short of, and the rounds end.
    assert_eq!(
        refused,
        [
            (
                1,
                vec!["Fill 1".into(), "Fill 3".into()],
                vec![small("Fill 1"), small("Fill 3")]
            ),
            all.clone(),
            all,
        ],
        "{said:?}"
    );
}

/// The big fill short of 1024 where the worker's other models hold
/// 8 GiB: refused at 1024, the card given back once across them, and
/// made at 1024, never at 512.
#[test]
fn a_big_fill_short_of_1024_with_room_elsewhere_runs_at_1024() {
    const GIB: u64 = 1 << 30;
    let mut edit = three_removes();
    edit.retouch.patches[1].radius = 0.25;
    let (said, gave, ran, _) = fills_said_at(
        "short-big-elsewhere",
        (1600, 1200),
        edit,
        (6 * GIB + GIB / 2, 0, 8 * GIB),
        None,
        None,
    );
    assert_eq!(ran, ran_at(1024), "{said:?}");
    assert_eq!(gave[0], 1, "{said:?}");
}

/// A queue holding the UNet at 512, a big fill after a small one: the
/// big one keeps the queue's UNet and is refused at 1024 rather than
/// run at 512 beside it; the worker's give-back lets the UNet go and
/// the big one, asked again, runs at 1024.
#[test]
fn a_queues_unet_at_512_gives_way_to_a_big_fill_at_1024() {
    const GIB: u64 = 1 << 30;
    let mut edit = three_removes();
    edit.retouch.patches[1].radius = 0.25;
    // 3 GiB free beside the UNet: 512 held needs 2.4, 1024 cold 7.1.
    let (said, gave, ran, _) = fills_said_at(
        "queue-unet",
        (1600, 1200),
        edit,
        (3 * GIB, 0, 0),
        Some((512, greycard_ai::remove::UNET_ON_CARD)),
        None,
    );
    assert_eq!(ran, ran_at(1024), "{said:?}");
    assert_eq!(gave[0], 1, "{said:?}");
}

/// A set of two frames the card stays short of gives the card back
/// across the worker's models once, for its first frame's second try;
/// its second frame asks once and is not tried again.
#[test]
fn a_set_short_of_the_card_gives_it_back_once() {
    const GIB: u64 = 1 << 30;
    let (dir, paths) = dngs("short-set", &["short-set.dng"], 128, 96);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::SetFrameDone { done, .. } = o {
            let failed = matches!(done, crate::queue::Done::Failed { .. });
            tx.lock().unwrap().send(failed).unwrap();
        }
    });
    worker.fill_by_test(0.9);
    worker.free_by_test(GIB, GIB, GIB);
    worker.send(Job::Open {
        path: paths[0].clone(),
        version: None,
        edit: Edit::default(),
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let set = Arc::new(crate::queue::Set::new(
        2,
        None,
        greycard_render::export::Settings::default(),
        greycard_render::export::OnExists::Overwrite,
    ));
    let frame = |i: usize| crate::queue::Frame {
        source: paths[0].clone(),
        edit: three_removes(),
        version: None,
        turn: 0,
        time_shift: 0,
        seed_blend: false,
        out: dir.join(format!("short-set-{i}.jpg")),
        moved_from: None,
    };
    worker.send(Job::ExportSet {
        set,
        frames: vec![frame(0), frame(1)],
    });
    for _ in 0..2 {
        let failed = rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("a frame done");
        assert!(failed, "a frame the card is short of fails");
    }
    assert_eq!(worker.gave_back_by_test(), 1);
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A set of two frames, each with a big removal the card is short of
/// at 1024 and nothing to give back: the first frame is refused at 1024
/// after its tries and tried again, where it runs at 512; the second,
/// its set met short, falls to 512 at once. Both are written.
#[test]
fn a_set_short_of_1024_alone_is_written_at_512() {
    const GIB: u64 = 1 << 30;
    let (dir, paths) = dngs("short-set-big", &["short-set-big.dng"], 1600, 1200);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::SetFrameDone { done, .. } = o {
            let failed = matches!(done, crate::queue::Done::Failed { .. });
            tx.lock().unwrap().send(failed).unwrap();
        }
    });
    worker.fill_by_test(0.9);
    worker.free_by_test(6 * GIB + GIB / 2, 0, 0);
    worker.send(Job::Open {
        path: paths[0].clone(),
        version: None,
        edit: Edit::default(),
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let set = Arc::new(crate::queue::Set::new(
        2,
        None,
        greycard_render::export::Settings::default(),
        greycard_render::export::OnExists::Overwrite,
    ));
    let mut edit = three_removes();
    edit.retouch.patches[1].radius = 0.25;
    let frame = |i: usize| crate::queue::Frame {
        source: paths[0].clone(),
        edit: edit.clone(),
        version: None,
        turn: 0,
        time_shift: 0,
        seed_blend: false,
        out: dir.join(format!("short-set-big-{i}.jpg")),
        moved_from: None,
    };
    worker.send(Job::ExportSet {
        set,
        frames: vec![frame(0), frame(1)],
    });
    for i in 0..2 {
        let failed = rx
            .recv_timeout(std::time::Duration::from_secs(120))
            .expect("a frame done");
        assert!(!failed, "frame {i} written at 512");
    }
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A set canceled while its frame runs, its big removal refused at
/// 1024 with nothing to give back: the frame is not tried again at 512,
/// and fails as a canceled frame short of the card does.
#[test]
fn a_canceled_set_is_not_tried_again_at_512() {
    const GIB: u64 = 1 << 30;
    let (dir, paths) = dngs("short-set-cancel", &["short-set-cancel.dng"], 1600, 1200);
    let set = Arc::new(crate::queue::Set::new(
        1,
        None,
        greycard_render::export::Settings::default(),
        greycard_render::export::OnExists::Overwrite,
    ));
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let canceling = set.clone();
    let worker = Worker::new(move |o| match o {
        // Canceled as its first fill starts.
        Outcome::Filling { .. } => canceling.cancel(),
        Outcome::SetFrameDone { done, .. } => tx.lock().unwrap().send(done).unwrap(),
        _ => {}
    });
    worker.fill_by_test(0.9);
    worker.free_by_test(6 * GIB + GIB / 2, 0, 0);
    worker.send(Job::Open {
        path: paths[0].clone(),
        version: None,
        edit: Edit::default(),
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let mut edit = three_removes();
    edit.retouch.patches[1].radius = 0.25;
    worker.send(Job::ExportSet {
        set,
        frames: vec![crate::queue::Frame {
            source: paths[0].clone(),
            edit,
            version: None,
            turn: 0,
            time_shift: 0,
            seed_blend: false,
            out: dir.join("short-set-cancel.jpg"),
            moved_from: None,
        }],
    });
    let done = rx
        .recv_timeout(std::time::Duration::from_secs(120))
        .expect("the frame done");
    assert!(
        matches!(done, crate::queue::Done::Failed { .. }),
        "{done:?}"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// Frames whose edit draws a Sky, each its raster in the mask cache as
/// an earlier visit left it, every texel of the value `names` gives, or
/// not there at all.
fn sky_frames(
    what: &str,
    names: &[(&str, Option<u8>)],
) -> (PathBuf, Vec<PathBuf>, Edit, greycard_ai::Store) {
    use greycard_edit::mask::Component;
    let files: Vec<&str> = names.iter().map(|(n, _)| *n).collect();
    let (dir, paths) = dngs(what, &files, 96, 64);
    let store = greycard_ai::Store::at(dir.join("models"));
    let sky = Shape::Sky { picks: Vec::new() };
    let mut edit = Edit::default();
    edit.adjustments.push(greycard_edit::Adjustment {
        id: 1,
        name: "Sky".into(),
        mask: greycard_edit::mask::Mask {
            components: vec![Component {
                shape: sky.clone(),
                ..Component::default()
            }],
            ..Default::default()
        },
        ..greycard_edit::Adjustment::default()
    });
    for (path, value) in paths
        .iter()
        .zip(names)
        .filter_map(|(p, (_, v))| Some((p, (*v)?)))
    {
        let print = crate::mask_base::print(Some(path), &edit, 0, crate::mask_base::Size::Full);
        let slot = Ai::for_test_on_disk(store.clone())
            .cached_path_of(path, &sky, 0, print)
            .expect("a slot in the mask cache");
        std::fs::create_dir_all(slot.parent().unwrap()).unwrap();
        let width = greycard_edit::brush::raster_width(&sky, (96, 64));
        let height = greycard_edit::brush::raster_height(width, 64.0 / 96.0);
        image::GrayImage::from_pixel(width as u32, height as u32, image::Luma([value]))
            .save_with_format(&slot, image::ImageFormat::Png)
            .unwrap();
    }
    (dir, paths, edit, store)
}

/// What a worker delivered on an open, in order: each develop, by its
/// generation, and each learned mask, by its key and whether a model
/// made it.
#[derive(Debug, Clone, PartialEq)]
enum Seen {
    Developed(u64),
    Mask(Key, bool),
    MaskFailed(Key),
}

fn seeing_worker(store: greycard_ai::Store) -> (Worker, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let worker = Worker::new(move |o| {
        let s = match o {
            Outcome::Developed { generation, .. } => Seen::Developed(generation),
            Outcome::Mask { key, provider, .. } => Seen::Mask(key, provider.is_some()),
            Outcome::MaskFailed { key, .. } => Seen::MaskFailed(key),
            _ => return,
        };
        log.lock().unwrap().push(s);
    });
    worker.models_by_test(store);
    (worker, seen)
}

/// An open of `path` under `edit`, the window asking at once for the
/// Sky at (1, 0) with the sky model, as its step names it.
fn open_with(path: &std::path::Path, edit: &Edit, generation: u64) -> Job {
    Job::Open {
        path: path.to_path_buf(),
        version: None,
        edit: edit.clone(),
        generation,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: vec![((1, 0), &greycard_ai::SKY)],
    }
}

/// A frame whose learned mask is made already draws it the first time:
/// its raster is read from the mask cache and delivered before the
/// open's first picture, no model run, so the window never draws the
/// frame without it and then with it. The opens' times are said, not
/// held to a bound: one sample each moves more than the read.
#[test]
fn a_learned_mask_in_the_cache_comes_before_the_first_picture() {
    let (dir, paths, edit, store) = sky_frames(
        "mask-first",
        &[
            ("warm.dng", None),
            ("made.dng", Some(255)),
            ("unmade.dng", None),
        ],
    );
    let (worker, seen) = seeing_worker(store);
    let mut took = Vec::new();
    for (generation, path) in (1..).zip(&paths) {
        seen.lock().unwrap().clear();
        let started = Instant::now();
        worker.send(open_with(path, &edit, generation));
        until("the open develops", &|| {
            seen.lock().unwrap().contains(&Seen::Developed(generation))
        });
        took.push(started.elapsed());
        let order = seen.lock().unwrap().clone();
        if generation == 2 {
            assert_eq!(
                order,
                vec![Seen::Mask((1, 0), false), Seen::Developed(2)],
                "the mask read from the cache, then the picture"
            );
        } else {
            assert_eq!(order, vec![Seen::Developed(generation)]);
        }
    }
    eprintln!(
        "opens: made {:.3} s, unmade {:.3} s",
        took[1].as_secs_f64(),
        took[2].as_secs_f64()
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A frame whose learned mask is not made yet opens as before: the
/// picture first, no mask with it and none asked for by the worker,
/// and the window's ask after it answered then.
#[test]
fn a_learned_mask_not_made_yet_is_asked_for_after_the_picture() {
    let (dir, paths, edit, store) = sky_frames("mask-after", &[("sky.dng", None)]);
    let (worker, seen) = seeing_worker(store);
    worker.send(open_with(&paths[0], &edit, 1));
    until("the open develops", &|| {
        seen.lock().unwrap().contains(&Seen::Developed(1))
    });
    // Past the settle, in which the worker would ask again for a mask
    // of a base it had given before.
    std::thread::sleep(masks::SETTLE + std::time::Duration::from_millis(200));
    assert_eq!(*seen.lock().unwrap(), vec![Seen::Developed(1)]);
    // The window's ask, which with no Sky model in the store fails.
    worker.send(Job::Mask {
        key: (1, 0),
        shape: Shape::Sky { picks: Vec::new() },
        model: None,
    });
    until("the ask is answered", &|| seen.lock().unwrap().len() == 2);
    assert_eq!(seen.lock().unwrap()[1], Seen::MaskFailed((1, 0)));
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A frame's learned masks held back from its first picture though
/// made: a Sky the window would offer its model for (not in the
/// open's list), and a Sky found empty, whose word for the status line
/// the picture's own would cover; each is asked for by the window
/// after the picture, as before.
#[test]
fn a_mask_offered_or_found_empty_waits_for_the_window_s_ask() {
    let (dir, paths, edit, store) = sky_frames(
        "mask-held",
        &[("offered.dng", Some(255)), ("empty.dng", Some(0))],
    );
    let (worker, seen) = seeing_worker(store);
    let Job::Open {
        path,
        version,
        edit: opened,
        generation,
        seed_blend,
        turn,
        stance_known,
        ..
    } = open_with(&paths[0], &edit, 1)
    else {
        unreachable!("an open")
    };
    worker.send(Job::Open {
        path,
        version,
        edit: opened,
        generation,
        seed_blend,
        turn,
        stance_known,
        learned: Vec::new(),
    });
    until("the open develops", &|| {
        seen.lock().unwrap().contains(&Seen::Developed(1))
    });
    assert_eq!(*seen.lock().unwrap(), vec![Seen::Developed(1)], "offered");
    seen.lock().unwrap().clear();
    worker.send(open_with(&paths[1], &edit, 2));
    until("the open develops", &|| {
        seen.lock().unwrap().contains(&Seen::Developed(2))
    });
    assert_eq!(*seen.lock().unwrap(), vec![Seen::Developed(2)], "empty");
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A mask answered ahead of the picture is recorded as given with the
/// model the window's step named, so a base moved after asks for it
/// again with that model, as the window's own ask would have; one the
/// window did not list is not read at all.
#[test]
fn a_mask_answered_with_the_open_is_given_with_the_window_s_model() {
    use greycard_edit::mask::Component;
    let frame = Arc::new(aberrated_frame(160, 120));
    let sky = Shape::Sky { picks: Vec::new() };
    let mut edit = Edit::default();
    edit.adjustments.push(greycard_edit::Adjustment {
        id: 1,
        mask: greycard_edit::mask::Mask {
            components: vec![
                Component {
                    shape: sky.clone(),
                    ..Component::default()
                },
                Component {
                    shape: sky.clone(),
                    ..Component::default()
                },
            ],
            ..Default::default()
        },
        ..greycard_edit::Adjustment::default()
    });
    let mut base = None;
    develop_with(&frame, &edit, &mut base, &mut None);
    let b = base.as_ref().expect("a base");
    let dir = crate::testing::scratch_dir("mask-given");
    let file = dir.join("given.dng");
    let mut ai = Ai::for_test_on_disk(greycard_ai::Store::at(dir.join("models")));
    let slot = ai
        .cached_path_of(&file, &sky, 0, b.print)
        .expect("a slot in the mask cache");
    std::fs::create_dir_all(slot.parent().unwrap()).unwrap();
    let size = (b.image.width, b.image.height);
    let width = greycard_edit::brush::raster_width(&sky, size);
    let height = greycard_edit::brush::raster_height(width, size.1 as f32 / size.0 as f32);
    image::GrayImage::from_pixel(width as u32, height as u32, image::Luma([255]))
        .save_with_format(&slot, image::ImageFormat::Png)
        .unwrap();
    let kept = masks::kept_for_open(
        &edit,
        &[((1, 1), &greycard_ai::SKY)],
        b,
        &mut ai,
        Some(&file),
    );
    assert_eq!(kept.len(), 1);
    assert!(matches!(kept[0], Outcome::Mask { key: (1, 1), .. }));
    assert_eq!(
        ai.given_model_by_test((1, 1)).flatten().map(|m| m.id),
        Some(greycard_ai::SKY.id)
    );
    assert_eq!(ai.given_model_by_test((1, 0)), None, "not listed, not read");
    crate::testing::remove_dir_retry(&dir);
}

/// A frame whose learned mask a fill lies over, made again on the
/// retouched picture on an earlier visit, draws it the first time it is
/// stepped back to: the raster made under its fills is read from the
/// mask cache and goes ahead of the picture, and nothing is asked again
/// after it, so the window never draws the frame without the mask and
/// then with it.
#[test]
fn a_mask_made_again_under_a_fill_comes_before_the_first_picture() {
    use greycard_edit::mask::Component;
    let (dir, paths) = dngs("mask-under-fill", &["filled.dng", "other.dng"], 128, 96);
    let store = greycard_ai::Store::at(dir.join("models"));
    let shape = Shape::Background {};
    let model = &greycard_ai::SUBJECT;
    let mut edit = Edit::default();
    edit.adjustments.push(greycard_edit::Adjustment {
        id: 1,
        name: "Background".into(),
        mask: greycard_edit::mask::Mask {
            components: vec![Component {
                shape: shape.clone(),
                ..Component::default()
            }],
            ..Default::default()
        },
        ..greycard_edit::Adjustment::default()
    });
    edit.retouch = Retouch {
        enabled: true,
        patches: vec![greycard_edit::retouch::Patch {
            id: 1,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.5, 0.5]],
            radius: 0.04,
            ..Default::default()
        }],
    };
    // The base's raster, every texel the shape, and the one made again
    // under the fill over it, as an earlier visit left them.
    let frame = (128, 96);
    let width = greycard_edit::brush::raster_width(&shape, frame);
    let height = greycard_edit::brush::raster_height(width, 96.0 / 128.0);
    let raster =
        greycard_edit::brush::Raster::from_data(96.0 / 128.0, width, vec![255; width * height]);
    let under = crate::ai::print_of(&crate::ai::overlapping(&edit.retouch, &raster));
    let print = crate::mask_base::print(Some(&paths[0]), &edit, 0, crate::mask_base::Size::Full);
    let mut ai = Ai::for_test_on_disk(store.clone());
    for (salt, value) in [(None, 255), (Some(under), 200)] {
        let slot = ai
            .cached_path_under(&paths[0], (&shape, Some(model)), frame, print, salt)
            .expect("a slot in the mask cache");
        std::fs::create_dir_all(slot.parent().unwrap()).unwrap();
        image::GrayImage::from_pixel(width as u32, height as u32, image::Luma([value]))
            .save_with_format(&slot, image::ImageFormat::Png)
            .unwrap();
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let worker = Worker::new(move |o| {
        let s = match o {
            Outcome::Developed {
                generation, fills, ..
            } => format!("developed {generation}: {:?}", fills.pending),
            Outcome::Mask { key, raster, .. } => {
                format!("mask {key:?}: {}", raster.data()[0])
            }
            Outcome::MaskFailed { key, .. } => format!("failed {key:?}"),
            _ => return,
        };
        log.lock().unwrap().push(s);
    });
    worker.models_by_test(store);
    worker.fill_by_test(0.9);
    let open = |path: &PathBuf, generation| Job::Open {
        path: path.clone(),
        version: None,
        edit: edit.clone(),
        generation,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: vec![((1, 0), model)],
    };
    let said = |what: &str| seen.lock().unwrap().iter().any(|s| s == what);
    // The first visit makes the fill and keeps it.
    worker.send(open(&paths[0], 1));
    until("the fill is made", &|| said("developed 1: []"));
    worker.send(open(&paths[1], 2));
    until("the step away develops", &|| said("developed 2: []"));
    std::thread::sleep(masks::SETTLE + std::time::Duration::from_millis(200));
    seen.lock().unwrap().clear();
    // Stepped back to.
    worker.send(open(&paths[0], 3));
    until("the step back develops", &|| said("developed 3: []"));
    std::thread::sleep(masks::SETTLE + std::time::Duration::from_millis(200));
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            "mask (1, 0): 200".to_string(),
            "developed 3: []".to_string()
        ],
        "the mask made under the fill, then the picture, and nothing after"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A develop aside, the before of a before and after, leaves the open
/// frame's own as it was: its base, the stamp the masks know it by,
/// the edit they see it through, and the develop it last delivered;
/// the frame's next develop goes on from the base kept.
#[test]
fn a_develop_aside_leaves_the_frames_own_develop() {
    use crate::stages::Keep;
    let frame = Arc::new(aberrated_frame(200, 160));
    let deliver: Deliver = Arc::new(|_| {});
    let mut gpu = None;
    let mut base = None;
    let mut edit = Edit::default();
    edit.light.exposure = 1.0;
    edit.noise.profiled = true;
    edit.noise.strength = 3.0;
    develop_with(&frame, &edit, &mut base, &mut gpu);
    let seen = |base: &Option<FrameKept>| {
        let b = base.as_ref().expect("the frame kept");
        (
            b.stamp,
            b.edit.clone(),
            b.developed_as.clone(),
            b.kept.picture(Keep::Base).cloned().expect("a base"),
        )
    };
    let before = seen(&base);
    // The before: the default edit, whose base is another.
    let outcome = develop_aside(
        &Input::Raw(frame.clone()),
        &Edit::default(),
        0,
        2,
        base.as_ref(),
        &mut Ai::new(),
        (None, None),
        &mut gpu,
        &deliver,
    );
    assert!(matches!(outcome, Outcome::Developed { generation: 2, .. }));
    let after = seen(&base);
    assert_eq!(after.0, before.0, "the stamp");
    assert_eq!(after.1, before.1, "the edit the masks see");
    assert_eq!(after.2, before.2, "the develop delivered");
    assert!(Arc::ptr_eq(&after.3, &before.3), "the base");
    // The frame's own next develop: the base kept, not made again.
    edit.light.exposure = 0.5;
    develop_with(&frame, &edit, &mut base, &mut gpu);
    let next = seen(&base);
    assert_eq!(next.0, before.0, "the base was made again");
    assert!(Arc::ptr_eq(&next.3, &before.3));
    // The before's base is another indeed: its own develop makes one.
    develop_with(&frame, &Edit::default(), &mut base, &mut gpu);
    assert_ne!(
        seen(&base).0,
        before.0,
        "the default edit's base is the same"
    );
}

/// A monochrome sensor's raw with no preview of its own gets a
/// thumbnail developed from its plane: grey, and the ramp across it.
#[test]
fn a_monochrome_raw_gets_a_grey_thumbnail() {
    let dir = crate::testing::scratch_dir("mono-thumb");
    let path = dir.join("mono.dng");
    crate::testing::monochrome_dng(&path, 96, 64);
    let thumb = thumbnail(&path, 48);
    crate::testing::remove_dir_retry(&dir);
    let (w, h, rgb) = thumb.unwrap();
    assert_eq!((w, h), (48, 32));
    for px in rgb.as_chunks::<3>().0 {
        assert!(
            px[0].abs_diff(px[1]) <= 1 && px[2].abs_diff(px[1]) <= 1,
            "{px:?}"
        );
    }
    let at = |x: u32| rgb[((h / 2 * w + x) * 3 + 1) as usize];
    assert!(at(w - 1) > at(0) + 50, "{} to {}", at(0), at(w - 1));
}

/// What a worker said of its opens and develops, in order: an open's
/// generation, a develop's with its picture's size and mean, a
/// failure's with its message.
#[derive(Debug, PartialEq)]
enum Said {
    Opened(u64),
    Developed(u64, (u32, u32), f32),
    Failed(u64, String),
}

/// A worker whose opens and develops are said on the channel returned.
fn saying() -> (Worker, std::sync::mpsc::Receiver<Said>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        let said = match o {
            Outcome::Opened { generation, .. } => Said::Opened(generation),
            Outcome::Developed {
                generation, image, ..
            } => {
                let Developed::Halves(h) = image else {
                    panic!("a test's worker has no device");
                };
                let rgb: Vec<f32> = h
                    .pixels
                    .chunks(4)
                    .flat_map(|p| [p[0], p[1], p[2]])
                    .map(|v| v.to_f32())
                    .collect();
                Said::Developed(
                    generation,
                    (h.width, h.height),
                    rgb.iter().sum::<f32>() / rgb.len() as f32,
                )
            }
            Outcome::Failed {
                generation,
                message,
            } => Said::Failed(generation, message),
            _ => return,
        };
        let _ = tx.lock().unwrap().send(said);
    });
    (worker, rx)
}

/// The next `n` things said, each within a minute.
fn next_said(rx: &std::sync::mpsc::Receiver<Said>, n: usize) -> Vec<Said> {
    (0..n)
        .map(|_| {
            rx.recv_timeout(std::time::Duration::from_secs(60))
                .expect("the worker says it")
        })
        .collect()
}

fn open_of(path: &std::path::Path, edit: Edit, generation: u64) -> Job {
    Job::Open {
        path: path.to_path_buf(),
        version: None,
        edit,
        generation,
        seed_blend: false,
        turn: 0,
        stance_known: true,
        learned: Vec::new(),
    }
}

fn develop_of(path: &std::path::Path, edit: Edit, generation: u64) -> Job {
    Job::Develop {
        edit,
        generation,
        turn: 0,
        of: Some((path.to_path_buf(), None)),
    }
}

/// An edit whose develop is not the default's: its Dehaze runs in the
/// engine, where the light's sliders are the viewport's.
fn hazed() -> Edit {
    let mut edit = Edit::default();
    edit.detail.enabled = true;
    edit.detail.dehaze = 0.8;
    edit
}

/// The open of `path` at `generation` sent, and held once it has read
/// the file: the sender returned lets it go on.
fn open_held(
    worker: &Worker,
    path: &std::path::Path,
    generation: u64,
) -> std::sync::mpsc::Sender<()> {
    let (held, go) = worker.hold_open_by_test();
    worker.send(open_of(path, Edit::default(), generation));
    held.recv_timeout(std::time::Duration::from_secs(60))
        .expect("the open reads its file");
    go
}

/// A develop of the frame asked for while its open reads the file (a
/// slider's debounce, the Original key, an undo) is that open: what the
/// open says of the frame lands at the develop's generation, which the
/// window waits on, and the one develop that runs is the newer edit's.
/// The open's neighbors and the open the worker says landed follow it.
#[test]
fn a_develop_asked_while_an_open_reads_its_file_is_that_open() {
    let (dir, paths) = dngs("open-read-fold", &["rf-A.dng"], 64, 48);
    let a = &paths[0];
    let (worker, said) = saying();
    let (held, go) = worker.hold_open_by_test();
    worker.send(open_of(a, Edit::default(), 1));
    worker.neighbors(1, Heading::steady(Side::Next), Vec::new());
    held.recv_timeout(std::time::Duration::from_secs(60))
        .expect("the open reads its file");
    worker.send(Job::Develop {
        edit: hazed(),
        generation: 2,
        turn: 1,
        of: Some((a.clone(), None)),
    });
    drop(go);
    let first = next_said(&said, 2);
    assert_eq!(first[0], Said::Opened(2), "{first:?}");
    // At the develop's turn, a quarter round.
    let Said::Developed(2, (48, 64), folded) = first[1] else {
        panic!("the develop at the newer generation: {first:?}");
    };
    {
        let q = worker.queue.0.lock().unwrap();
        assert_eq!(q.landed, Some((2, a.clone())));
        assert_eq!(q.folded, Some((1, 2)));
    }
    // The picture is the newer edit's: that edit's developed again, not
    // the open's own.
    worker.send(develop_of(a, Edit::default(), 3));
    let own = next_said(&said, 1);
    worker.send(develop_of(a, hazed(), 4));
    let again = next_said(&said, 1);
    let ([Said::Developed(3, _, own)], [Said::Developed(4, _, again)]) = (&own[..], &again[..])
    else {
        panic!("nothing else came between: {own:?} {again:?}");
    };
    assert!((folded - again).abs() < 1e-5, "{folded} against {again}");
    assert!((again - own).abs() > 1e-3, "{again} against {own}");
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// An open that fails with a develop of its frame asked for while it
/// read the file says so at the develop's generation; a develop or an
/// aside of that frame asked after is answered with the same failure,
/// and never made from the file the worker still has open, another
/// frame's. A later open that succeeds develops as before.
#[test]
fn a_develop_of_a_frame_whose_open_failed_is_that_failure() {
    let (dir, paths) = dngs("open-read-failed", &["rf-ok.dng"], 64, 48);
    let a = &paths[0];
    let junk = dir.join("rf-junk.CR3");
    std::fs::write(&junk, vec![0x33u8; 4096]).unwrap();
    let (worker, said) = saying();
    worker.send(open_of(a, Edit::default(), 1));
    let first = next_said(&said, 2);
    assert!(
        matches!(first[..], [Said::Opened(1), Said::Developed(1, ..)]),
        "{first:?}"
    );
    let go = open_held(&worker, &junk, 2);
    worker.send(develop_of(&junk, hazed(), 3));
    drop(go);
    let Said::Failed(3, message) = next_said(&said, 1).remove(0) else {
        panic!("the failure at the develop's generation");
    };
    worker.send(develop_of(&junk, Edit::default(), 4));
    assert_eq!(next_said(&said, 1), [Said::Failed(4, message.clone())]);
    worker.send(Job::DevelopAside {
        edit: Edit::default(),
        generation: 5,
        turn: 0,
        of: Some((junk.clone(), None)),
    });
    assert_eq!(next_said(&said, 1), [Said::Failed(5, message)]);
    worker.send(open_of(a, Edit::default(), 6));
    let back = next_said(&said, 2);
    assert!(
        matches!(back[..], [Said::Opened(6), Said::Developed(6, ..)]),
        "{back:?}"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// Two opens in quick succession: the second, and a develop of its
/// frame folded into it as it waits, are not the first's, which reads
/// its file meanwhile and lands at its own generation.
#[test]
fn two_opens_in_quick_succession_land_each_at_its_own() {
    let (dir, paths) = dngs("open-read-two", &["rf-1.dng", "rf-2.dng"], 64, 48);
    let (worker, said) = saying();
    let go = open_held(&worker, &paths[0], 1);
    worker.send(open_of(&paths[1], Edit::default(), 2));
    worker.send(develop_of(&paths[1], hazed(), 3));
    drop(go);
    let all = next_said(&said, 4);
    assert!(
        matches!(
            all[..],
            [
                Said::Opened(1),
                Said::Developed(1, ..),
                Said::Opened(3),
                Said::Developed(3, ..)
            ]
        ),
        "{all:?}"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// Another version of the file being opened is another frame: a
/// develop of it asked while the open reads is not folded in.
#[test]
fn an_open_reading_its_file_takes_no_develop_of_another_version() {
    let (dir, paths) = dngs("open-read-version", &["rf-v.dng"], 64, 48);
    let a = &paths[0];
    let (worker, said) = saying();
    let go = open_held(&worker, a, 1);
    worker.send(Job::Develop {
        edit: hazed(),
        generation: 2,
        turn: 0,
        of: Some((a.clone(), Some("v1".into()))),
    });
    drop(go);
    let all = next_said(&said, 3);
    assert!(
        matches!(
            all[..],
            [
                Said::Opened(1),
                Said::Developed(1, ..),
                Said::Developed(2, ..)
            ]
        ),
        "{all:?}"
    );
    assert_eq!(worker.queue.0.lock().unwrap().folded, None);
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A develop of a file the worker has not been sent an open of (a pick
/// still reading its sidecar, a frame under an offline root), asked
/// while an open of another reads its file: not that open's, which
/// lands at its own generation, and never made from the file in hand.
#[test]
fn an_open_reading_its_file_takes_no_develop_of_another_frame() {
    let (dir, paths) = dngs("open-read-other", &["ro-A.dng"], 64, 48);
    let a = &paths[0];
    let other = dir.join("ro-other.CR3");
    let (worker, said) = saying();
    let go = open_held(&worker, a, 1);
    worker.send(develop_of(&other, hazed(), 2));
    drop(go);
    let all = next_said(&said, 2);
    assert!(
        matches!(all[..], [Said::Opened(1), Said::Developed(1, ..)]),
        "{all:?}"
    );
    assert_eq!(worker.queue.0.lock().unwrap().folded, None);
    // The other's develop made nothing: the next said is A's own.
    worker.send(develop_of(&other, hazed(), 3));
    worker.send(develop_of(a, Edit::default(), 4));
    let next = next_said(&said, 1);
    assert!(matches!(next[..], [Said::Developed(4, ..)]), "{next:?}");
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// The panic's words, as the window is told them.
fn panicked_with(what: &str) -> String {
    super::panicked(what)
}

/// A decoder that panics while a develop of its frame is asked is a
/// failed open at the develop's generation, in the words of a panic;
/// a develop of that frame after it is the same failure, and the file
/// still in hand, another frame's, is never developed for it.
#[test]
fn a_decoder_that_panics_with_a_develop_asked_fails_the_open() {
    let (dir, paths) = dngs("open-panic-race", &["pr-A.dng", "pr-B.dng"], 64, 48);
    let (worker, said) = saying();
    worker.send(open_of(&paths[0], Edit::default(), 1));
    let first = next_said(&said, 2);
    assert!(
        matches!(first[..], [Said::Opened(1), Said::Developed(1, ..)]),
        "{first:?}"
    );
    worker.open_panics_by_test(super::OpenPanic::Reading);
    let go = open_held(&worker, &paths[1], 2);
    worker.send(develop_of(&paths[1], hazed(), 3));
    drop(go);
    let message = panicked_with("a test's decoder");
    assert_eq!(next_said(&said, 1), [Said::Failed(3, message.clone())]);
    worker.send(develop_of(&paths[1], Edit::default(), 4));
    assert_eq!(next_said(&said, 1), [Said::Failed(4, message)]);
    assert_eq!(worker.queue.0.lock().unwrap().landed, None);
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// Without the race: a decoder that panics fails its open, and a
/// develop of that frame after it is the same failure.
#[test]
fn a_decoder_that_panics_is_a_failed_open() {
    let (dir, paths) = dngs("open-panic", &["pn-A.dng", "pn-B.dng"], 64, 48);
    let (worker, said) = saying();
    worker.send(open_of(&paths[0], Edit::default(), 1));
    let first = next_said(&said, 2);
    assert!(
        matches!(first[..], [Said::Opened(1), Said::Developed(1, ..)]),
        "{first:?}"
    );
    worker.open_panics_by_test(super::OpenPanic::Reading);
    worker.send(open_of(&paths[1], Edit::default(), 2));
    let message = panicked_with("a test's decoder");
    assert_eq!(next_said(&said, 1), [Said::Failed(2, message.clone())]);
    worker.send(develop_of(&paths[1], Edit::default(), 3));
    assert_eq!(next_said(&said, 1), [Said::Failed(3, message)]);
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// An open that panics after its read, a develop of its frame taken in,
/// and before its file is the worker's (its profile, its base): the
/// panic is the develop's generation's, and the frame's develops after
/// it are that failure, not the last frame's picture.
#[test]
fn an_open_that_panics_after_its_read_is_a_failed_open() {
    let (dir, paths) = dngs("open-panic-late", &["pl-A.dng", "pl-B.dng"], 64, 48);
    let (worker, said) = saying();
    worker.send(open_of(&paths[0], Edit::default(), 1));
    let first = next_said(&said, 2);
    assert!(
        matches!(first[..], [Said::Opened(1), Said::Developed(1, ..)]),
        "{first:?}"
    );
    worker.open_panics_by_test(super::OpenPanic::Landing);
    let go = open_held(&worker, &paths[1], 2);
    worker.send(develop_of(&paths[1], hazed(), 3));
    drop(go);
    let message = panicked_with("a test's open, after its read");
    assert_eq!(next_said(&said, 1), [Said::Failed(3, message.clone())]);
    worker.send(develop_of(&paths[1], Edit::default(), 4));
    assert_eq!(next_said(&said, 1), [Said::Failed(4, message)]);
    // The worker goes on: the first frame opens again.
    worker.send(open_of(&paths[0], Edit::default(), 5));
    let back = next_said(&said, 2);
    assert!(
        matches!(back[..], [Said::Opened(5), Said::Developed(5, ..)]),
        "{back:?}"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// The failure of an open is remembered for the frame until it opens:
/// its file written again and read again (the window's re-read, an
/// open of the same file) clears it, and the frame develops.
#[test]
fn a_failed_open_read_again_develops() {
    let dir = crate::testing::scratch_dir("open-failed-again");
    let path = dir.join("fa.dng");
    std::fs::write(&path, vec![0x33u8; 4096]).unwrap();
    let (worker, said) = saying();
    worker.send(open_of(&path, Edit::default(), 1));
    let Said::Failed(1, _) = next_said(&said, 1).remove(0) else {
        panic!("junk fails");
    };
    worker.send(develop_of(&path, Edit::default(), 2));
    assert!(matches!(next_said(&said, 1)[..], [Said::Failed(2, _)]));
    crate::testing::linear_dng(&path, 64, 48, "Test", "Cam");
    worker.send(open_of(&path, Edit::default(), 3));
    let again = next_said(&said, 2);
    assert!(
        matches!(again[..], [Said::Opened(3), Said::Developed(3, ..)]),
        "{again:?}"
    );
    worker.send(develop_of(&path, Edit::default(), 4));
    assert!(matches!(next_said(&said, 1)[..], [Said::Developed(4, ..)]));
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// The frame's file moved on disk and the window followed it: what the
/// window then sends is the frame opened again under the new path
/// (`browser::reopen_moved`). A develop still waiting under the old
/// path gives way to that open in the slot, which sets every path the
/// worker keeps of the frame; nothing is left waiting on an answer.
#[test]
fn a_develop_waiting_when_its_file_moves_gives_way_to_the_open_again() {
    let (dir, paths) = dngs("moved-develop", &["md-A.dng"], 64, 48);
    let (a, to) = (&paths[0], dir.join("md-A-moved.dng"));
    let (worker, said) = saying();
    let go = open_held(&worker, a, 1);
    worker.send(develop_of(a, hazed(), 2));
    std::fs::rename(a, &to).unwrap();
    worker.send(open_of(&to, Edit::default(), 3));
    drop(go);
    let all = next_said(&said, 4);
    assert!(
        matches!(
            all[..],
            [
                Said::Opened(1),
                Said::Developed(1, ..),
                Said::Opened(3),
                Said::Developed(3, ..)
            ]
        ),
        "{all:?}"
    );
    assert_eq!(worker.queue.0.lock().unwrap().landed, Some((3, to.clone())));
    worker.send(develop_of(&to, Edit::default(), 4));
    assert!(matches!(next_said(&said, 1)[..], [Said::Developed(4, ..)]));
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// An open of the frame still waiting when its file moves gives way to
/// the open under the new path: the old path, gone, is never read, so
/// no failure is remembered under it.
#[test]
fn an_open_waiting_when_its_file_moves_gives_way_to_the_open_again() {
    let (dir, paths) = dngs("moved-open", &["mo-B.dng", "mo-A.dng"], 64, 48);
    let (b, a, to) = (&paths[0], &paths[1], dir.join("mo-A-moved.dng"));
    let (worker, said) = saying();
    let go = open_held(&worker, b, 1);
    worker.send(open_of(a, Edit::default(), 2));
    std::fs::rename(a, &to).unwrap();
    worker.send(open_of(&to, Edit::default(), 3));
    drop(go);
    let all = next_said(&said, 4);
    assert!(
        matches!(
            all[..],
            [
                Said::Opened(1),
                Said::Developed(1, ..),
                Said::Opened(3),
                Said::Developed(3, ..)
            ]
        ),
        "{all:?}"
    );
    worker.send(develop_of(&to, Edit::default(), 4));
    assert!(matches!(next_said(&said, 1)[..], [Said::Developed(4, ..)]));
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// An open running when its file moves finishes on what it read and
/// says so at its own generation, which the window, waiting on the open
/// again, refuses; that open then sets the new path, and the frame
/// develops under it.
#[test]
fn an_open_running_when_its_file_moves_is_followed_by_the_open_again() {
    let (dir, paths) = dngs("moved-running", &["mr-A.dng"], 64, 48);
    let (a, to) = (&paths[0], dir.join("mr-A-moved.dng"));
    let (worker, said) = saying();
    let go = open_held(&worker, a, 1);
    std::fs::rename(a, &to).unwrap();
    worker.send(open_of(&to, Edit::default(), 2));
    drop(go);
    let all = next_said(&said, 4);
    assert!(
        matches!(
            all[..],
            [
                Said::Opened(1),
                Said::Developed(1, ..),
                Said::Opened(2),
                Said::Developed(2, ..)
            ]
        ),
        "{all:?}"
    );
    worker.send(develop_of(&to, Edit::default(), 3));
    assert!(matches!(next_said(&said, 1)[..], [Said::Developed(3, ..)]));
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A white balance off the default, which a base is made under.
fn white_at(temperature: f64) -> Edit {
    Edit {
        white_balance: greycard_edit::WhiteBalance::Custom {
            temperature,
            tint: 0.0,
        },
        ..Edit::default()
    }
}

/// The sidecar of `raw` written beside it, its own line under `edit`
/// and the frame at `turn`.
fn write_sidecar(raw: &std::path::Path, edit: Edit, turn: u8) {
    let mut sidecar = greycard_edit::Sidecar {
        line: greycard_edit::Line::of(edit, 0).unwrap(),
        turn,
        ..greycard_edit::Sidecar::default()
    };
    sidecar
        .save_in(raw, greycard_edit::Placement::Beside)
        .expect("the sidecar written");
}

/// A neighbor the window sends to be read first: the placeholders a
/// stand-in's row gives, and the cell's version.
fn read_first_at(path: &std::path::Path, version: Option<&str>, side: Side) -> Neighbor {
    Neighbor {
        read_first: Some(super::ReadFirst {
            version: version.map(Arc::from),
        }),
        ..neighbor_at(path, Edit::default(), side)
    }
}

/// `read_first` with nothing remembered of sidecars that did not serve.
fn peek(n: &Neighbor, asked: &dyn Fn() -> bool) -> super::prefetch::Peeked {
    super::prefetch::read_first(n, &mut super::prefetch::Peeks::default(), asked)
}

/// Every file in `dir` with its bytes and its modified time.
fn folder_as_it_is(dir: &std::path::Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("the folder")
        .map(|e| {
            let path = e.expect("an entry").path();
            let bytes = std::fs::read(&path).expect("its bytes");
            let modified = path
                .metadata()
                .and_then(|m| m.modified())
                .expect("its time");
            (path, bytes, modified)
        })
        .collect();
    files.sort();
    files
}

/// The open of `path` under `edit` at `turn` as the worker makes it
/// after `shelf` (the key from the file and its decode, the base taken
/// back or not), developed; whether the base came from the shelf, and
/// the picture beside a fresh develop's of the same file.
fn open_after(
    path: &std::path::Path,
    edit: &Edit,
    turn: u8,
    shelf: &mut super::shelf::Shelf,
) -> (bool, Arc<WorkingImage>, Arc<WorkingImage>) {
    use super::engine::swap_base;
    use super::shelf::FileKey;
    let (input, _) = open(path).expect("our DNG decodes");
    let key = FileKey::of(path).expect("on the disk").decoded(&input);
    let mut base = None;
    swap_base(
        &mut base,
        shelf,
        (None, Some((&key, &input))),
        (edit, turn, &|_, _| false),
        None,
        None,
    );
    let from_shelf = base.as_ref().is_some_and(|b| b.from_shelf);
    let deliver: Deliver = Arc::new(|_| {});
    let develop = |base: &mut Option<FrameKept>| {
        develop_job(
            &input,
            edit,
            turn,
            1,
            base,
            &mut Ai::new(),
            None,
            None,
            &mut None,
            &deliver,
        )
        .1
        .expect("a picture on the CPU")
    };
    let back = develop(&mut base);
    let fresh = develop(&mut None);
    (from_shelf, back, fresh)
}

/// A neighbor whose edit the window does not hold is read first: the
/// worker takes the edit, the turn, and the seed from its sidecar, makes
/// its base under them, and the open of it, under the edit the window's
/// own read then gives, takes that base from the shelf and develops to
/// the picture a fresh develop makes, to the bit. Nothing is written:
/// the folder, an XMP with a Lightroom develop beside the sidecar
/// included, is as it was to the byte and the time.
#[test]
fn an_edited_neighbor_is_read_first_and_its_open_takes_the_base() {
    use super::prefetch::{Ahead, Fetched, Peeked, prefetch, said};
    use super::shelf::{Budget, Shelf};
    let (dir, paths) = dngs("prefetch-read-first", &["pf-rf-B.dng"], 96, 64);
    let b = &paths[0];
    let warm = white_at(6500.0);
    write_sidecar(b, warm.clone(), 1);
    // A Lightroom develop beside it, which the window's reader would
    // take over a sidecar with no edit of ours, and write.
    std::fs::write(
        greycard_edit_io::xmp::short_path(b),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="3" crs:ProcessVersion="11.0" crs:WhiteBalance="As Shot"
   crs:Exposure2012="+1.00" crs:HasSettings="True"/>
 </rdf:RDF>
</x:xmpmeta>"#,
    )
    .expect("the XMP written");
    let before = folder_as_it_is(&dir);

    let n = read_first_at(b, None, Side::Next);
    let Peeked::Ready(ready) = peek(&n, &|| false) else {
        panic!("read");
    };
    assert_eq!(ready.edit, warm, "the sidecar's edit, not the placeholder");
    assert_eq!(ready.turn, 1);
    assert!(!ready.seed_blend, "an edited line is not seeded");
    assert!(ready.read_first.is_none());
    assert!(
        said("pf-rf-B.dng")
            .iter()
            .any(|l| l.starts_with("the next frame, pf-rf-B.dng: its sidecar read first, in ")),
        "{:?}",
        said("pf-rf-B.dng")
    );
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    assert_eq!(
        prefetch(&ready, None, &[], &mut shelf, &mut None, None, &|_| {
            Ahead::Nothing
        }),
        Fetched::Landed
    );
    assert_eq!(shelf.len(), 1);
    assert_eq!(folder_as_it_is(&dir), before, "nothing written");

    let (from_shelf, back, fresh) = open_after(b, &warm, 1, &mut shelf);
    assert!(from_shelf, "taken back");
    assert!(back.data == fresh.data, "bit for bit a fresh develop's");
    // Made under the placeholder, as before the read, it would have
    // been another base.
    let mut placeholder = Shelf::new(Budget::Bytes(1 << 30));
    prefetch(&n, None, &[], &mut placeholder, &mut None, None, &|_| {
        Ahead::Nothing
    });
    assert!(!open_after(b, &warm, 1, &mut placeholder).0);
    assert_eq!(folder_as_it_is(&dir), before, "nothing written");
    crate::testing::remove_dir_retry(&dir);
}

/// The same frame with a sidecar holding no edit of ours and the same
/// Lightroom develop beside it: the window's reader takes the develop
/// and writes the sidecar; the worker's read writes nothing.
#[test]
fn the_read_first_takes_no_lightroom_develop() {
    use super::prefetch::Peeked;
    let (dir, paths) = dngs("prefetch-read-lr", &["pf-rf-lr-B.dng"], 64, 48);
    let b = &paths[0];
    write_sidecar(b, Edit::default(), 0);
    std::fs::write(
        greycard_edit_io::xmp::short_path(b),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:ProcessVersion="11.0" crs:WhiteBalance="As Shot"
   crs:Exposure2012="+1.00" crs:HasSettings="True"/>
 </rdf:RDF>
</x:xmpmeta>"#,
    )
    .expect("the XMP written");
    let before = folder_as_it_is(&dir);
    assert!(matches!(
        peek(&read_first_at(b, None, Side::Next), &|| false),
        Peeked::Ready(_)
    ));
    assert_eq!(folder_as_it_is(&dir), before, "nothing written");
    // The window's reader, for the contrast: it writes.
    greycard_edit_io::on_disk::load_sidecar_said(
        b,
        Some(greycard_edit::Placement::Beside),
        greycard_edit_io::on_disk::Fresh::default(),
        |_, _| None,
    );
    assert_ne!(folder_as_it_is(&dir), before, "the window's read takes it");
    crate::testing::remove_dir_retry(&dir);
}

/// A virtual copy's cell is read first from its own line: the version's
/// edit, not the file's own, and its open takes the base.
#[test]
fn a_virtual_copy_s_line_is_the_one_read_first() {
    use super::prefetch::{Ahead, Fetched, Peeked, prefetch};
    use super::shelf::{Budget, Shelf};
    let (dir, paths) = dngs("prefetch-read-copy", &["pf-rf-copy-B.dng"], 96, 64);
    let b = &paths[0];
    let (cool, warm) = (white_at(4000.0), white_at(6500.0));
    let mut sidecar = greycard_edit::Sidecar {
        line: greycard_edit::Line::of(cool.clone(), 0).unwrap(),
        ..greycard_edit::Sidecar::default()
    };
    let id = sidecar
        .new_version(None, Some("Warm"))
        .expect("a version made");
    sidecar
        .line_mut(Some(&id))
        .expect("the version")
        .record(warm.clone());
    sidecar
        .save_in(b, greycard_edit::Placement::Beside)
        .expect("written");

    let read = |version: Option<&str>| match peek(&read_first_at(b, version, Side::Next), &|| false)
    {
        Peeked::Ready(ready) => ready,
        other => panic!("{other:?}"),
    };
    assert_eq!(read(None).edit, cool, "the file's own line");
    let copy = read(Some(&id));
    assert_eq!(copy.edit, warm, "the version's line");
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    assert_eq!(
        prefetch(&copy, None, &[], &mut shelf, &mut None, None, &|_| {
            Ahead::Nothing
        }),
        Fetched::Landed
    );
    let (from_shelf, back, fresh) = open_after(b, &warm, 0, &mut shelf);
    assert!(from_shelf);
    assert!(back.data == fresh.data);
    crate::testing::remove_dir_retry(&dir);
}

/// A sidecar that does not read, one that is not there, or one without
/// the cell's version is skipped and said; nothing is decoded or made.
#[test]
fn an_unreadable_sidecar_is_skipped() {
    use super::prefetch::{Peeked, said};
    let (dir, paths) = dngs("prefetch-read-bad", &["pf-rf-bad-B.dng"], 64, 48);
    let b = &paths[0];
    let n = read_first_at(b, None, Side::Previous);
    assert!(matches!(
        peek(&n, &|| false),
        Peeked::Skipped(why) if why == "its sidecar is not there"
    ));
    let gcd = greycard_edit::Sidecar::path_in(b, greycard_edit::Placement::Beside);
    std::fs::write(&gcd, b"{ not json").expect("written");
    let before = folder_as_it_is(&dir);
    assert!(matches!(
        peek(&n, &|| false),
        Peeked::Skipped(why) if why.starts_with("its sidecar does not read: ")
    ));
    assert!(
        said("pf-rf-bad-B.dng").iter().any(|l| l.starts_with(
            "the prefetch of the previous frame, pf-rf-bad-B.dng, skipped: \
             its sidecar does not read: "
        )),
        "{:?}",
        said("pf-rf-bad-B.dng")
    );
    assert_eq!(folder_as_it_is(&dir), before, "never written over");
    write_sidecar(b, white_at(6500.0), 0);
    assert!(matches!(
        peek(&read_first_at(b, Some("gone"), Side::Previous), &|| false),
        Peeked::Skipped(why) if why == "its sidecar has no such version"
    ));
    crate::testing::remove_dir_retry(&dir);
}

/// A sidecar whose edit asks for the learned denoiser is read, and then
/// skipped as any such neighbor is.
#[test]
fn a_learned_edit_read_first_is_skipped() {
    use super::prefetch::{Ahead, Fetched, Peeked, prefetch};
    use super::shelf::{Budget, Shelf};
    let (dir, paths) = dngs("prefetch-read-learned", &["pf-rf-ln-B.dng"], 64, 48);
    let b = &paths[0];
    let learned = Edit {
        noise: Noise {
            learned: greycard_edit::Learned::Fast,
            ..Noise::default()
        },
        ..Edit::default()
    };
    write_sidecar(b, learned, 0);
    let Peeked::Ready(ready) = peek(&read_first_at(b, None, Side::Next), &|| false) else {
        panic!("read");
    };
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    assert_eq!(
        prefetch(&ready, None, &[], &mut shelf, &mut None, None, &|_| {
            Ahead::Nothing
        }),
        Fetched::Skipped("its edit asks for the learned denoiser".into())
    );
    assert_eq!(shelf.len(), 0);
    crate::testing::remove_dir_retry(&dir);
}

/// A sidecar changed between the worker's read and the open: the base
/// made under the old edit (read, and shelved under it) is not the
/// open's, which develops afresh, to the picture a fresh develop of the
/// new edit makes.
#[test]
fn a_sidecar_changed_after_the_read_misses_the_shelf() {
    use super::prefetch::{Ahead, Fetched, Peeked, prefetch};
    use super::shelf::{Budget, FileKey, Shelf};
    let (dir, paths) = dngs("prefetch-read-moved", &["pf-rf-mv-B.dng"], 96, 64);
    let b = &paths[0];
    let warm = white_at(6500.0);
    write_sidecar(b, warm.clone(), 0);
    let Peeked::Ready(ready) = peek(&read_first_at(b, None, Side::Next), &|| false) else {
        panic!("read");
    };
    assert_eq!(ready.edit, warm, "read");
    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    assert_eq!(
        prefetch(&ready, None, &[], &mut shelf, &mut None, None, &|_| {
            Ahead::Nothing
        }),
        Fetched::Landed
    );
    let stat = FileKey::of(b).expect("on the disk").stat;
    assert!(
        shelf.holds((b, &stat), &warm, false, 0, false, None),
        "the old sidecar's base shelved"
    );
    // Another hand writes it; the window's read at the open gives this.
    let cool = white_at(4000.0);
    write_sidecar(b, cool.clone(), 0);
    let (from_shelf, back, fresh) = open_after(b, &cool, 0, &mut shelf);
    assert!(!from_shelf, "the old edit's base is not handed out");
    assert!(back.data == fresh.data);
    crate::testing::remove_dir_retry(&dir);
}

/// The read gives way to anything asked of the worker, before it reads
/// (nothing read) and after (before the decode).
#[test]
fn the_read_first_gives_way_before_and_after_the_read() {
    use super::prefetch::{Peeked, said};
    let (dir, paths) = dngs("prefetch-read-way", &["pf-rf-way-B.dng"], 64, 48);
    let b = &paths[0];
    write_sidecar(b, white_at(6500.0), 0);
    let n = read_first_at(b, None, Side::Next);
    assert!(matches!(peek(&n, &|| true), Peeked::GaveWay));
    assert!(
        !said("pf-rf-way-B.dng")
            .iter()
            .any(|l| l.contains("read first")),
        "not read"
    );
    let calls = std::cell::Cell::new(0);
    let second = || {
        calls.set(calls.get() + 1);
        calls.get() == 2
    };
    assert!(matches!(peek(&n, &second), Peeked::GaveWay));
    assert_eq!(calls.get(), 2);
    let lines = said("pf-rf-way-B.dng");
    assert!(lines.iter().any(|l| l
        == "the prefetch of the next frame, pf-rf-way-B.dng, abandoned before its sidecar's read: \
            the worker was asked for something"));
    assert!(lines.iter().any(|l| l
        == "the prefetch of the next frame, pf-rf-way-B.dng, abandoned after its sidecar's read: \
            the worker was asked for something"));
    crate::testing::remove_dir_retry(&dir);
}

/// What the read asks is anything at all queued, the open of any frame
/// and a save among them; nothing queued, nothing.
#[test]
fn the_read_first_asks_whether_anything_is_queued() {
    use super::prefetch::asked;
    let mut q = Queue::default();
    assert!(!asked(&q, true));
    assert!(asked(&q, false), "the editor left");
    q.develop = Some(open_default(std::path::Path::new("b.dng"), 2));
    assert!(asked(&q, true), "an open, even of its own frame");
    q.develop = None;
    q.keeps.push_back(Job::Keep {
        frame: Frame::file("a.dng"),
        edit: Edit::default(),
        turn: 0,
        iso: None,
    });
    assert!(asked(&q, true), "a save");
    q.keeps.clear();
    q.mask_picture = Some(Job::MaskPicture {
        edit: Box::new(Edit::default()),
        generation: 1,
    });
    assert!(asked(&q, true), "the refined masks' picture");
    q.mask_picture = None;
    q.gpu_lost = Some(true);
    assert!(asked(&q, true), "the device lost");
    q.gpu_lost = None;
    // Masks settling: not while they wait, once they are due.
    q.settling = Some(super::masks::Settling {
        due: Instant::now() + std::time::Duration::from_secs(60),
        asks: Vec::new(),
    });
    assert!(!asked(&q, true), "settling");
    q.settling = Some(super::masks::Settling {
        due: Instant::now() - std::time::Duration::from_millis(1),
        asks: Vec::new(),
    });
    assert!(asked(&q, true), "settled");
}

/// A line never developed is read first with its seed still to come,
/// as the window's open asks once it has read it: the file's own at the
/// default, at any turn (a mirrored row's), and a version made from the
/// original before the raw was opened. An edited version is not.
#[test]
fn a_line_never_developed_is_read_first_with_its_seed_to_come() {
    use super::prefetch::Peeked;
    let (dir, paths) = dngs("prefetch-read-seed", &["pf-rf-seed-B.dng"], 64, 48);
    let b = &paths[0];
    let mut sidecar = greycard_edit::Sidecar {
        turn: 2,
        ..greycard_edit::Sidecar::default()
    };
    let fresh = sidecar
        .new_version_from_original(Edit::default(), Some("Fresh"))
        .unwrap();
    let edited = sidecar
        .new_version(None, Some("Warm"))
        .expect("a version made");
    sidecar
        .line_mut(Some(&edited))
        .expect("the version")
        .record(white_at(6500.0));
    sidecar
        .save_in(b, greycard_edit::Placement::Beside)
        .expect("written");
    let read = |version: Option<&str>| match peek(&read_first_at(b, version, Side::Next), &|| false)
    {
        Peeked::Ready(ready) => (ready.seed_blend, ready.turn),
        other => panic!("{other:?}"),
    };
    assert_eq!(read(None), (true, 2), "the file's own, turned");
    assert_eq!(read(Some(&fresh)), (true, 2), "a version never developed");
    assert_eq!(read(Some(&edited)), (false, 2), "an edited version");
    crate::testing::remove_dir_retry(&dir);
}

/// A sidecar that did not serve is said once and not read again while
/// it stands as it was; written anew, it is read again.
#[test]
fn an_unreadable_sidecar_is_not_read_again_until_it_changes() {
    use super::prefetch::{Peeked, Peeks, read_first, said};
    let (dir, paths) = dngs("prefetch-read-once", &["pf-rf-once-B.dng"], 64, 48);
    let b = &paths[0];
    let gcd = greycard_edit::Sidecar::path_in(b, greycard_edit::Placement::Beside);
    std::fs::write(&gcd, b"{ not json").expect("written");
    let n = read_first_at(b, None, Side::Next);
    let mut unread = Peeks::default();
    let skips = || {
        said("pf-rf-once-B.dng")
            .iter()
            .filter(|l| l.contains("skipped: its sidecar does not read"))
            .count()
    };
    for _ in 0..3 {
        assert!(matches!(
            read_first(&n, &mut unread, &|| false),
            Peeked::Skipped(_)
        ));
    }
    assert_eq!(skips(), 1, "said once");
    // Written anew, by another hand: read, and taken.
    std::fs::remove_file(&gcd).expect("removed");
    write_sidecar(b, white_at(6500.0), 0);
    assert!(matches!(
        read_first(&n, &mut unread, &|| false),
        Peeked::Ready(r) if r.edit == white_at(6500.0)
    ));
    crate::testing::remove_dir_retry(&dir);
}

/// A neighbor in a folder the window's check says is on an automount
/// not mounted yet, or (once the share is mounted) on a network mount, a
/// folder opened outside any root among them, is not touched at all: no
/// sidecar looked for, none read. Said once a folder. One in a local
/// folder is read.
#[test]
fn a_neighbor_on_a_network_mount_is_not_read_first() {
    use super::prefetch::{Peeked, Peeks, read_first, said, touches};
    let (dir, paths) = dngs(
        "prefetch-read-nfs",
        &["pf-rf-nfs-B.dng", "pf-rf-nfs-C.dng"],
        64,
        48,
    );
    write_sidecar(&paths[0], white_at(6500.0), 0);
    write_sidecar(&paths[1], white_at(6500.0), 0);
    thread_local! {
        static SHARE: std::cell::RefCell<(PathBuf, &'static str)> =
            const { std::cell::RefCell::new((PathBuf::new(), "")) };
    }
    fn on_share(folder: &std::path::Path) -> Option<String> {
        SHARE.with(|s| {
            let (share, what) = &*s.borrow();
            (share == folder).then(|| what.to_string())
        })
    }
    SHARE.with(|s| *s.borrow_mut() = (dir.clone(), "an automount not mounted yet"));
    let mut peeks = Peeks::default();
    peeks.set_remote(on_share);
    let before = touches();
    assert!(matches!(
        read_first(&read_first_at(&paths[0], None, Side::Next), &mut peeks, &|| false),
        Peeked::Skipped(why) if why == "its folder is on an automount not mounted yet"
    ));
    // The share mounted by another hand: seen at once.
    SHARE.with(|s| s.borrow_mut().1 = "a network mount (nfs4)");
    for p in &paths {
        assert!(matches!(
            read_first(&read_first_at(p, None, Side::Next), &mut peeks, &|| false),
            Peeked::Skipped(why) if why == "its folder is on a network mount (nfs4)"
        ));
    }
    assert_eq!(touches(), before, "nothing of the frames touched");
    let lines: Vec<String> = ["pf-rf-nfs-B.dng", "pf-rf-nfs-C.dng"]
        .iter()
        .flat_map(|n| said(n))
        .collect();
    assert_eq!(
        lines,
        vec![
            "the prefetch of the next frame, pf-rf-nfs-B.dng, skipped: \
             its folder is on an automount not mounted yet"
        ],
        "said once a folder, nothing read"
    );
    // The folder local: read.
    SHARE.with(|s| s.borrow_mut().0 = PathBuf::from("/elsewhere"));
    assert!(matches!(
        read_first(
            &read_first_at(&paths[0], None, Side::Next),
            &mut peeks,
            &|| false
        ),
        Peeked::Ready(_)
    ));
    assert!(touches() > before, "read");
    crate::testing::remove_dir_retry(&dir);
}

/// The worker asks the check handed over (`Worker::set_remote_check`)
/// of a read-first neighbor's folder before it reads it, and passes the
/// neighbor over when the check says so.
#[test]
fn the_worker_asks_the_check_it_was_handed_of_a_read_first_neighbor() {
    use super::prefetch::{QUIET, said};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ASKED: AtomicUsize = AtomicUsize::new(0);
    fn on_share(_folder: &std::path::Path) -> Option<String> {
        ASKED.fetch_add(1, Ordering::SeqCst);
        Some("a network mount (nfs4)".into())
    }
    let (dir, paths) = dngs(
        "prefetch-read-check",
        &["pf-rf-chk-A.dng", "pf-rf-chk-B.dng"],
        64,
        48,
    );
    write_sidecar(&paths[1], white_at(6500.0), 0);
    let (worker, rx) = walking_worker();
    worker.set_remote_check(on_share);
    assert_eq!(worker.remote_checks().0, 1);
    worker.send(open_default(&paths[0], 1));
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![read_first_at(&paths[1], None, Side::Next)],
    );
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(60))
            .expect("the open lands"),
        1
    );
    until("B passed over", &|| {
        said("pf-rf-chk-B.dng")
            .iter()
            .any(|l| l.ends_with("its folder is on a network mount (nfs4)"))
    });
    std::thread::sleep(QUIET * 10);
    assert!(ASKED.load(Ordering::SeqCst) >= 1);
    assert!(
        !said("pf-rf-chk-B.dng")
            .iter()
            .any(|l| l.contains("read first") || l.starts_with("prefetched")),
        "never read or made"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A device lost while the worker is idle, not yet taken up: the open
/// frame's neighbors wait, none begun and given way to over and over.
#[test]
fn the_worker_begins_no_prefetch_while_a_lost_device_waits() {
    use super::prefetch::{QUIET, said};
    let (dir, paths) = dngs("prefetch-lost", &["pf-lost-A.dng", "pf-lost-B.dng"], 64, 48);
    let (worker, rx) = walking_worker();
    worker.send(open_default(&paths[0], 1));
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(60))
            .expect("the open lands"),
        1
    );
    // The open's develop is delivered before the worker is back at its
    // wait; a loss told in between is taken up at once, and the
    // prefetch then begins as it should. So let it settle first.
    std::thread::sleep(QUIET * 4);
    (worker.gpu_lost_teller())(false);
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![neighbor_at(&paths[1], Edit::default(), Side::Next)],
    );
    std::thread::sleep(QUIET * 20);
    assert!(said("pf-lost-B.dng").is_empty(), "not begun");
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A read-first neighbor that gave way after its read (to the refined
/// masks' picture asked meanwhile) goes back on its list, is read again
/// once the worker is quiet, and is made.
#[test]
fn a_read_first_neighbor_that_gave_way_is_read_again_and_made() {
    use super::prefetch::{AtCheck, at_next_check, said};
    let (dir, paths) = dngs(
        "prefetch-read-again",
        &["pf-rf-again-A.dng", "pf-rf-again-B.dng"],
        96,
        64,
    );
    write_sidecar(&paths[1], white_at(6500.0), 0);
    let (at_tx, at_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    at_next_check(&paths[1], AtCheck::Hold(at_tx, go_rx));
    let (worker, rx) = walking_worker();
    let wait = std::time::Duration::from_secs(60);
    worker.send(open_default(&paths[0], 1));
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![read_first_at(&paths[1], None, Side::Next)],
    );
    assert_eq!(rx.recv_timeout(wait).expect("the open lands"), 1);
    assert_eq!(at_rx.recv_timeout(wait).expect("held"), "sidecar's read");
    worker.send(Job::MaskPicture {
        edit: Box::new(Edit::default()),
        generation: 1,
    });
    go_tx.send(()).unwrap();
    until("B made after giving way", &|| {
        said("pf-rf-again-B.dng")
            .iter()
            .any(|l| l.starts_with("prefetched pf-rf-again-B.dng"))
    });
    let lines = said("pf-rf-again-B.dng");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("abandoned after its sidecar's read")),
        "{lines:?}"
    );
    assert_eq!(
        lines.iter().filter(|l| l.contains("read first")).count(),
        2,
        "read again: {lines:?}"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// The worker on its thread reads an edited neighbor first in its idle
/// slot and makes its base; the open of it, under the sidecar's edit and
/// turn, takes the base from the shelf.
#[test]
fn the_worker_reads_an_edited_neighbor_first_and_its_open_takes_the_base() {
    use super::prefetch::said;
    let (dir, paths) = dngs(
        "prefetch-read-run",
        &["pf-rf-run-A.dng", "pf-rf-run-B.dng"],
        96,
        64,
    );
    let warm = white_at(6500.0);
    write_sidecar(&paths[1], warm.clone(), 1);
    let (worker, rx) = walking_worker();
    let wait = std::time::Duration::from_secs(60);
    worker.send(open_default(&paths[0], 1));
    worker.neighbors(
        1,
        Heading::steady(Side::Next),
        vec![read_first_at(&paths[1], None, Side::Next)],
    );
    assert_eq!(rx.recv_timeout(wait).expect("the open lands"), 1);
    until("B prefetched", &|| {
        said("pf-rf-run-B.dng")
            .iter()
            .any(|l| l.starts_with("prefetched pf-rf-run-B.dng"))
    });
    worker.send(Job::Open {
        path: paths[1].clone(),
        version: None,
        edit: warm,
        generation: 2,
        seed_blend: false,
        turn: 1,
        stance_known: false,
        learned: Vec::new(),
    });
    assert_eq!(rx.recv_timeout(wait).expect("B lands"), 2);
    assert!(
        said("pf-rf-run-B.dng")
            .iter()
            .any(|l| l == "the base of pf-rf-run-B.dng from the shelf"),
        "{:?}",
        said("pf-rf-run-B.dng")
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A save sent before the develop of its edit waits for it, whatever
/// the worker does between: another job meanwhile (the masks' picture
/// asked) does not let it go, and the develop's landing, on the device
/// here, keeps the edit's pictures.
#[test]
fn a_save_waits_for_its_develop_past_another_job() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("a save waiting on the device");
        return;
    };
    let rig = crate::testing::edited::rig("waits");
    rig.worker.set_context(ctx);
    let edited = rig.worker.edited();
    // Clarity: a develop of its own, which the open's does not stand
    // for.
    let mut clearer = Edit::for_picture();
    clearer.detail.clarity = 0.4;
    let hash = greycard_library::hash_file(&rig.png).unwrap();
    let key = edited.key_of(&rig.png, &clearer);
    rig.save(&clearer, 0);
    rig.worker.send(crate::worker::Job::MaskPicture {
        edit: Box::new(clearer.clone()),
        generation: 1,
    });
    while !matches!(rig.next(), crate::testing::edited::Heard::MaskPicture) {}
    rig.worker.send(crate::worker::Job::Develop {
        edit: clearer,
        generation: 2,
        turn: 0,
        of: None,
    });
    rig.kept();
    assert!(edited.all_kept_by(&hash, key, crate::edited_pictures::EDITED_RECIPE));
    rig.quiet();
}
