use super::*;

/// An X-Trans frame asked for the learned denoiser is told it does not
/// run, before anything is prepared; a Bayer one is not, and the
/// profiled denoise runs on both.
#[test]
fn the_learned_denoiser_on_x_trans_is_unavailable_and_said_so() {
    use greycard_core::raw::{CfaPattern, LevelPattern, Levels, Samples, SensorLayout};
    let mut frame = RawFrame {
        make: "Test".into(),
        model: "Cam".into(),
        width: 2,
        height: 2,
        channels: 1,
        layout: SensorLayout::Cfa(CfaPattern::rggb()),
        samples: Samples::U16(vec![0; 4]),
        levels: Levels {
            black: LevelPattern::uniform(0.0, 1),
            white: LevelPattern::uniform(1.0, 1),
        },
        as_shot_coefficients: None,
        calibrations: Vec::new(),
        crop: None,
        orientation: greycard_core::raw::Orientation::Normal,
        shot: Default::default(),
    };
    assert_eq!(denoise_unavailable(&frame, true), None);
    assert_eq!(denoise_unavailable(&frame, false), None);
    frame.layout = SensorLayout::Cfa(CfaPattern::xtrans());
    assert!(denoise_unavailable(&frame, true).is_some());
    assert_eq!(denoise_unavailable(&frame, false), None);
}

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
        crate::export::Settings::default(),
        crate::export::OnExists::Increment,
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
        width,
        height,
        channels: 1,
        layout: SensorLayout::Cfa(pattern),
        samples: Samples::U16(samples),
        levels: Levels {
            black: LevelPattern::uniform(100.0, 1),
            white: LevelPattern::uniform(1100.0, 1),
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
    }
}

/// `develop_job` on `frame` under the default edit with `gpu`,
/// from `base`: the outcome and the picture.
fn develop_once(
    frame: &Arc<RawFrame>,
    base: &mut Option<Base>,
    gpu: &mut Option<greycard_gpu::Context>,
) -> (Outcome, Option<Arc<WorkingImage>>) {
    let deliver: Deliver = Arc::new(|_| {});
    develop_job(
        &Input::Raw(frame.clone()),
        &Edit::default(),
        0,
        1,
        base,
        &mut None,
        &mut Ai::new(),
        None,
        None,
        gpu,
        &deliver,
    )
}

/// `develop_job` on `frame` under `edit` at the frame's `turn`,
/// on the CPU, from `base`: the picture.
fn develop_turned(
    frame: &Arc<RawFrame>,
    edit: &Edit,
    turn: u8,
    base: &mut Option<Base>,
) -> Arc<WorkingImage> {
    let deliver: Deliver = Arc::new(|_| {});
    let (outcome, image) = develop_job(
        &Input::Raw(frame.clone()),
        edit,
        turn,
        1,
        base,
        &mut None,
        &mut Ai::new(),
        None,
        None,
        &mut None,
        &deliver,
    );
    assert!(matches!(outcome, Outcome::Developed { .. }));
    image.expect("a picture on the CPU")
}

/// A base turned where it stands is the base the engine makes at
/// the new turn, to the bit, and so is its guide plane: for a
/// frame whose tag is a plain turn and one whose tag mirrors, for
/// a quarter each way and a half. And a develop that finds a base
/// at another turn turns it rather than making it again, and
/// develops what a fresh one would.
#[test]
fn a_base_turned_in_place_is_the_base_developed_at_that_turn() {
    use greycard_core::raw::Orientation;
    let edit = Edit::default();
    assert!(
        turns_exactly(&edit, None),
        "the check would not reach the turn"
    );
    for tag in [Orientation::Normal, Orientation::FlipHorizontal] {
        let mut frame = aberrated_frame(120, 80);
        frame.orientation = tag;
        let frame = Arc::new(frame);
        for (from, to) in [(0u8, 1u8), (1, 0), (0, 3), (1, 3), (2, 0)] {
            let mut kept = None;
            develop_turned(&frame, &edit, from, &mut kept);
            let mut b = kept.take().expect("a base");
            let stamp = b.stamp;
            turn_base(&mut b, &mut None, to, stamp + 1, None);
            let mut fresh = None;
            develop_turned(&frame, &edit, to, &mut fresh);
            let fresh = fresh.expect("a base");
            let what = format!("{tag:?}, {from} to {to}");
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
            assert_eq!((b.turn, b.stamp), (to, stamp + 1));
            assert!(b.patched.is_none() && b.pre.is_none());

            // Through the develop: the base found at `from` is
            // turned and kept, not made again, and the develop is
            // the fresh one's.
            let mut base = None;
            develop_turned(&frame, &edit, from, &mut base);
            let made = base.as_ref().map(|b| b.stamp);
            let image = develop_turned(&frame, &edit, to, &mut base);
            let b = base.as_ref().expect("a base");
            assert_eq!(b.turn, to);
            assert_eq!(Some(b.stamp), made.map(|s| s + 1));
            let reference = develop_turned(&frame, &edit, to, &mut None);
            assert!(image.data == reference.data, "{what}: the develop differs");
        }
    }
}

/// The learned denoiser's pair turns with the base: at either end
/// of the strength the base is one of the pair itself, and stays
/// shared with it once both are turned rather than turned twice
/// or copied; in between it is a blend of its own, turned. A pair
/// made at another turn than the base's is not the base's, and is
/// left for `learned_base` to make again.
#[test]
fn a_turned_base_takes_the_learned_pair_with_it() {
    use greycard_core::develop::orient;
    use greycard_core::raw::Orientation;
    let frame = Arc::new(aberrated_frame(120, 80));
    let edit = Edit::default();
    let mut made = None;
    develop_turned(&frame, &edit, 0, &mut made);
    let base = made.expect("a base");
    let plain = base.image.clone();
    let model = Arc::new(WorkingImage {
        data: plain.data.iter().map(|v| v * 0.5 + 0.1).collect(),
        ..(*plain).clone()
    });
    let pair = |turn: u8| LearnedBase {
        edit: edit.clone(),
        turn,
        model: Arc::new((*model).clone()),
        plain: Arc::new((*plain).clone()),
        white: WhiteBase::IDENTITY,
        radius: None,
        clip_level: 1.0,
        ca_on_gpu: false,
    };
    let turned = |image: &WorkingImage| orient(image.clone(), Orientation::Rotate90);
    for strength in [0.0f32, 0.5, 1.0] {
        let mut learned = Some(pair(0));
        let l = learned.as_ref().unwrap();
        let image = blend(&l.model, &l.plain, strength);
        let expected = turned(&image);
        let mut b = Base {
            edit: edit.clone(),
            turn: 0,
            image,
            guide: Arc::new(crate::finish::Guide::NONE),
            white: WhiteBase::IDENTITY,
            radius: None,
            clip_level: 1.0,
            source: crate::finish::Source::SCENE,
            stamp: 1,
            patched: None,
            pre: None,
            uploaded: None,
            haze: None,
            planes: None,
            on_gpu: false,
            stand_in: None,
            gave_way: None,
            profile: None,
            from_shelf: false,
        };
        turn_base(&mut b, &mut learned, 1, 2, None);
        let l = learned.as_ref().unwrap();
        assert_eq!(l.turn, 1, "strength {strength}");
        assert!(
            l.model.data == turned(&model).data,
            "strength {strength}: the model"
        );
        assert!(
            l.plain.data == turned(&plain).data,
            "strength {strength}: the plain"
        );
        assert_eq!((b.image.width, b.image.height), (80, 120));
        assert!(
            b.image.data == expected.data,
            "strength {strength}: the base"
        );
        match strength {
            1.0 => assert!(Arc::ptr_eq(&b.image, &l.model), "the model, shared"),
            0.0 => assert!(Arc::ptr_eq(&b.image, &l.plain), "the plain, shared"),
            _ => assert!(!Arc::ptr_eq(&b.image, &l.model) && !Arc::ptr_eq(&b.image, &l.plain)),
        }
    }
    // A pair at another turn stays as it was.
    let mut learned = Some(pair(3));
    let mut b = Base {
        edit: edit.clone(),
        turn: 0,
        image: plain.clone(),
        guide: Arc::new(crate::finish::Guide::NONE),
        white: WhiteBase::IDENTITY,
        radius: None,
        clip_level: 1.0,
        source: crate::finish::Source::SCENE,
        stamp: 1,
        patched: None,
        pre: None,
        uploaded: None,
        haze: None,
        planes: None,
        on_gpu: false,
        stand_in: None,
        gave_way: None,
        profile: None,
        from_shelf: false,
    };
    turn_base(&mut b, &mut learned, 1, 2, None);
    let l = learned.as_ref().unwrap();
    assert_eq!(l.turn, 3);
    assert!(l.plain.data == plain.data && l.model.data == model.data);
    assert!(b.image.data == turned(&plain).data);
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

/// The export's picture is the reference's: a base whose CA ran on
/// the GPU is not reused by a develop without one (the export's),
/// which makes the base afresh on the CPU, and that picture is the
/// one a session that never had a GPU makes, byte for byte.
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
    // The export: no GPU, the same edit.
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

/// The planes' thread can end three ways, and the develop lands
/// without the bands in two of them: the maker declined (it sent
/// `None`), or the thread died without sending (a panic, which
/// drops the sender). Both leave nothing kept for the next develop,
/// so neither can serve a stale base; the second is said in the log
/// as a warning, where the first says its reason itself.
#[test]
fn a_planes_thread_that_declines_or_dies_lands_the_develop_without_them() {
    let image = Arc::new(WorkingImage::new(4, 4));
    let mut base = Base {
        edit: Edit::default(),
        turn: 0,
        image: image.clone(),
        guide: Arc::new(crate::finish::Guide::NONE),
        white: WhiteBase::IDENTITY,
        radius: None,
        clip_level: 1.0,
        source: crate::finish::Source::SCENE,
        stamp: 0,
        patched: None,
        pre: None,
        uploaded: None,
        haze: None,
        planes: None,
        on_gpu: false,
        stand_in: None,
        gave_way: None,
        profile: None,
        from_shelf: false,
    };
    let wants = greycard_edit::DetailWants {
        texture: true,
        clarity: false,
        haze: false,
    };
    let making = |made| PlanesJob::Making {
        made,
        _cancel: CancelOnDrop(Arc::new(std::sync::atomic::AtomicBool::new(false))),
        patched: image.clone(),
        wants,
        haze_after: None,
        started: Instant::now(),
    };

    // Declined: the maker sent None.
    let (send, made) = std::sync::mpsc::channel();
    send.send(None).unwrap();
    let mut job = making(made);
    assert!(job.land(&mut base).is_none());
    assert!(
        base.planes.is_none(),
        "nothing kept to serve the next develop"
    );
    assert!(matches!(job, PlanesJob::Nothing));

    // Died: the sender dropped without a word, as a panic drops it.
    let (send, made) = std::sync::mpsc::channel::<Option<crate::detail::DetailPlanes>>();
    drop(send);
    let mut job = making(made);
    assert!(job.land(&mut base).is_none());
    assert!(base.planes.is_none());
    assert!(matches!(job, PlanesJob::Nothing));
}

/// A learned pair whose CA ran on the GPU serves a develop with a
/// GPU and not one without, which makes the pair again on the CPU;
/// a pair whose CA was the CPU's serves both. The turn and the
/// learned settings still have to match.
#[test]
fn a_gpu_ca_learned_pair_is_not_served_to_an_export() {
    let edit = Edit::default();
    let image = Arc::new(WorkingImage::new(4, 4));
    let pair = |ca_on_gpu: bool| LearnedBase {
        edit: edit.clone(),
        turn: 0,
        model: image.clone(),
        plain: image.clone(),
        white: WhiteBase::IDENTITY,
        radius: None,
        clip_level: 1.0,
        ca_on_gpu,
    };
    let gpu = pair(true);
    assert!(pair_serves(&gpu, 0, &edit, true));
    assert!(
        !pair_serves(&gpu, 0, &edit, false),
        "the export makes it again"
    );
    let cpu = pair(false);
    assert!(pair_serves(&cpu, 0, &edit, true));
    assert!(pair_serves(&cpu, 0, &edit, false));
    assert!(!pair_serves(&cpu, 1, &edit, false), "another turn");
    let mut other = edit.clone();
    other.noise.learned = greycard_edit::Learned::Best;
    assert!(
        !pair_serves(&cpu, 0, &other, false),
        "other learned settings"
    );
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
    let image = develop_turned(&frame, &global, 0, &mut base);
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
        },
        look: greycard_edit::Look {
            white_balance: white,
            ..Default::default()
        },
        ..Default::default()
    };
    let settings = crate::export::Settings {
        format: crate::export::Format::Tiff,
        sharpen: crate::export::Sharpen::Off,
        ..Default::default()
    };
    let mut ai = Ai::new();
    let mut render = |white| {
        let mut edit = global.clone();
        edit.adjustments.push(everywhere(white));
        let (r, _) = finish_export(image.clone(), &edit, base.as_ref(), &mut ai, &settings);
        let crate::export::Pixels::Sixteen(p) = r.pixels else {
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
        },
        look: greycard_edit::Look {
            white_balance: white,
            ..Default::default()
        },
        ..Default::default()
    };
    let settings = crate::export::Settings {
        format: crate::export::Format::Tiff,
        sharpen: crate::export::Sharpen::Off,
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
        let image = develop_turned(&frame, &global, 0, &mut base);
        let mut edit = global.clone();
        edit.adjustments.push(everywhere(white));
        let (r, _) = finish_export(image, &edit, base.as_ref(), &mut ai, &settings);
        let crate::export::Pixels::Sixteen(p) = r.pixels else {
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

/// A last develop whose CA ran on the GPU is not handed to an export:
/// it develops afresh on the CPU, and that picture is then reused.
#[test]
fn an_export_does_not_reuse_a_gpu_ca_develop() {
    let frame = Arc::new(aberrated_frame(400, 320));
    // The reference, from nothing.
    let (_, reference) = develop_once(&frame, &mut None, &mut None);
    let reference = reference.unwrap();
    // A base as a GPU session leaves it, and a last develop from it
    // that is plainly not the reference: a flat grey picture.
    let mut base = None;
    let (_, image) = develop_once(&frame, &mut base, &mut None);
    let image = image.unwrap();
    base.as_mut().unwrap().on_gpu = true;
    let wrong = Arc::new(
        WorkingImage::from_data(image.width, image.height, vec![0.5; image.data.len()]).unwrap(),
    );
    let mut last = Some(Last::made(
        Edit::default(),
        base.as_ref().unwrap(),
        wrong.clone(),
        &Outcome::Failed {
            generation: 0,
            message: String::new(),
        },
    ));
    let mut learned = None;
    let mut ai = Ai::new();
    let deliver: Deliver = Arc::new(|_| {});
    let input = Input::Raw(frame.clone());
    let mut export = |last: &mut Option<Last>, base: &mut Option<Base>, ai: &mut Ai| {
        open_picture(
            &Edit::default(),
            0,
            Some(&input),
            last,
            base,
            &mut learned,
            ai,
            None,
            None,
            &deliver,
        )
        .unwrap()
        .0
    };
    let got = export(&mut last, &mut base, &mut ai);
    assert!(!Arc::ptr_eq(&got, &wrong), "the GPU develop was reused");
    assert_eq!(
        got.data, reference.data,
        "the export's picture is the reference's"
    );
    assert!(
        last.as_ref().is_some_and(|l| !l.on_gpu),
        "the last develop is the CPU's now"
    );
    assert!(base.as_ref().is_some_and(|b| !b.on_gpu));
    // Now that it is the CPU's, it is reused.
    let again = export(&mut last, &mut base, &mut ai);
    assert!(Arc::ptr_eq(&got, &again), "the CPU develop is kept");
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
    let (outcome, image) = develop_once(&frame, &mut base, &mut gpu);
    assert!(matches!(outcome, Outcome::Developed { .. }));
    assert!(gpu.is_none(), "the context is dropped after a device error");
    assert!(base.as_ref().is_some_and(|b| !b.on_gpu));
    let (_, reference) = develop_once(&frame, &mut None, &mut None);
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
    let (outcome, _) = develop_once(&frame, &mut base, &mut gpu);
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

/// A fresh base as the CPU makes it before its lens step, `frame`
/// under `edit`, for the step to be run on by each side.
fn base_before_lens(frame: &Arc<RawFrame>, edit: &Edit) -> Base {
    let mut before = edit.clone();
    before.lens.enabled = false;
    let mut base = None;
    develop_turned(frame, &before, 0, &mut base);
    let mut b = base.expect("a base");
    b.edit = edit.clone();
    b
}

/// The lens step of a fresh base on `gpu`: the base after it, the
/// stage it timed, and the device's error when it had one.
fn lens_stepped(
    frame: &Arc<RawFrame>,
    edit: &Edit,
    gpu: Option<&greycard_gpu::Context>,
) -> (Base, Stages, Option<greycard_gpu::Error>) {
    let mut b = base_before_lens(frame, edit);
    let mut stages = Stages::new();
    let failed = super::develop::lens_step(
        &mut b,
        &Input::Raw(frame.clone()),
        edit,
        None,
        gpu,
        &mut stages,
    );
    (b, stages, failed)
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
        }),
    }
}

/// `develop_job` on `input` under `edit`: the picture, when it came
/// back on the CPU.
fn develop_picture(
    input: &Input,
    edit: &Edit,
    base: &mut Option<Base>,
    gpu: &mut Option<greycard_gpu::Context>,
) -> Option<Arc<WorkingImage>> {
    let deliver: Deliver = Arc::new(|_| {});
    let (outcome, image) = develop_job(
        input,
        edit,
        0,
        1,
        base,
        &mut None,
        &mut Ai::new(),
        None,
        None,
        gpu,
        &deliver,
    );
    assert!(matches!(outcome, Outcome::Developed { .. }));
    image
}

/// The base's lens step on the GPU is the reference's within the GPU
/// op's own tolerance (greycard-gpu's lens tests say why 1e-4 of a
/// value in 0 to 1), the stage is timed as "lens" either way, and a
/// base it ran in says so, so the export makes its own.
#[test]
fn a_base_with_the_gpu_lens_is_the_cpu_s() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the GPU lens step");
        return;
    };
    let frame = Arc::new(aberrated_frame(400, 320));
    let edit = lens_edit();
    let (gpu, gpu_stages, failed) = lens_stepped(&frame, &edit, Some(&ctx));
    assert!(failed.is_none());
    let (cpu, cpu_stages, _) = lens_stepped(&frame, &edit, None);
    assert!(gpu.on_gpu && !cpu.on_gpu);
    for stages in [&gpu_stages, &cpu_stages] {
        assert_eq!(stages.iter().filter(|(n, _)| *n == "lens").count(), 1);
    }
    let before = base_before_lens(&frame, &edit);
    assert_ne!(
        cpu.image.data, before.image.data,
        "the lens moved something"
    );
    let scale = cpu.image.data.iter().fold(1f32, |m, v| m.max(v.abs()));
    let worst = cpu
        .image
        .data
        .iter()
        .zip(&gpu.image.data)
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    assert!(worst <= 1e-4 * scale, "{worst} apart, values up to {scale}");

    // A whole develop with the GPU marks its base, and the export's
    // develop, which has none, makes the reference's afresh. On a
    // picture that is not a raw, which has no CA correction, so only
    // the lens can have marked it.
    let input = picture_input(&before.image);
    let develop = |base: &mut Option<Base>, gpu: &mut Option<greycard_gpu::Context>| {
        develop_picture(&input, &edit, base, gpu)
    };
    let mut gpu = Some(ctx);
    let mut base = None;
    develop(&mut base, &mut gpu);
    assert!(base.as_ref().is_some_and(|b| b.on_gpu));
    let exported = develop(&mut base, &mut None).expect("a picture on the CPU");
    assert!(base.as_ref().is_some_and(|b| !b.on_gpu));
    let reference = develop(&mut None, &mut None).expect("a picture on the CPU");
    assert_eq!(exported.data, reference.data);
}

/// A device error in the lens step falls back to the reference for
/// that base, the base the CPU's to the bit and not marked, and is
/// returned; a develop then drops the context for the session, as it
/// does after the CA's. A picture the device cannot hold falls back
/// the same way and is not an error: the context stays. A device
/// allowed 20 workgroups a dimension cannot run the 400x320's 25
/// across; one whose textures stop at 256 a side cannot hold it.
#[test]
fn a_gpu_error_in_the_lens_falls_back_and_an_unsupported_size_keeps_the_context() {
    use greycard_gpu::wgpu::Limits;
    let frame = Arc::new(aberrated_frame(400, 320));
    let edit = lens_edit();
    let (cpu, _, _) = lens_stepped(&frame, &edit, None);
    let small = WorkingImage::new(8, 8);
    let identity = greycard_core::develop::lens::LensCorrection::default();
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
        let (b, stages, failed) = lens_stepped(&frame, &edit, Some(&ctx));
        assert!(!b.on_gpu);
        assert_eq!(b.image.data, cpu.image.data);
        assert!(stages.iter().any(|(n, _)| *n == "lens"));
        assert_eq!(failed.is_some(), device_error, "{failed:?} on {limits:?}");
        // The step leaves the context for its caller to decide on.
        assert!(ctx.correct_lens(&small, &identity).is_ok());
        // A whole develop of a picture that is not a raw, where only
        // the lens would run on the device, drops it after a device
        // error and keeps it after a size it declines.
        // The sharpen off, so that no op but the lens asks the device
        // for anything; without the lens the same develop keeps it.
        let input = picture_input(&base_before_lens(&frame, &edit).image);
        let mut quiet = Edit::default();
        quiet.sharpen.enabled = false;
        let mut gpu = Some(ctx);
        develop_picture(&input, &quiet, &mut None, &mut gpu);
        assert!(gpu.is_some(), "{limits:?}");
        let mut lensed = edit.clone();
        lensed.sharpen.enabled = false;
        let mut base = None;
        develop_picture(&input, &lensed, &mut base, &mut gpu);
        assert!(base.as_ref().is_some_and(|b| !b.on_gpu));
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

fn detail_section_on_the_gpu(ctx: greycard_gpu::Context, keep: bool) {
    let frame = Arc::new(aberrated_frame(400, 320));
    let deliver: Deliver = Arc::new(|_| {});
    let mut gpu = Some(ctx);
    let mut base = None;
    let develop = |edit: &Edit, base: &mut Option<Base>, gpu: &mut Option<_>| {
        develop_job(
            &Input::Raw(frame.clone()),
            edit,
            0,
            1,
            base,
            &mut None,
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
        .uploaded
        .as_ref()
        .expect("the patched picture is kept")
        .1
        .clone();
    assert!(b.pre.as_ref().is_some_and(|p| p.detail_stats.is_some()));
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
    assert!(Arc::ptr_eq(&b.uploaded.as_ref().unwrap().1, &uploaded));
    assert!(b.pre.as_ref().is_some_and(|p| p.detail == edit.detail));
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
    assert!(Arc::ptr_eq(&b.pre.as_ref().unwrap().image, &uploaded));
    assert_eq!(kept(&gpu).local_contrast, 0, "no Detail, no planes");
    // The dehaze after the local contrast, both on the GPU from the
    // kept upload; its reduced copy kept with what it was made from.
    edit.detail.clarity = -0.5;
    develop(&edit, &mut base, &mut gpu);
    assert_eq!(kept(&gpu).local_contrast, planes(4));
    assert!(base.as_ref().unwrap().haze.is_none());
    edit.detail.dehaze = 0.5;
    let (outcome, _) = develop(&edit, &mut base, &mut gpu);
    let Outcome::Developed {
        image: Developed::Texture(_),
        detail: Some((_, DetailRan::Gpu)),
        dehaze: Some(first),
        ..
    } = outcome
    else {
        panic!("the local contrast and the dehaze on the GPU");
    };
    let b = base.as_ref().unwrap();
    assert!(Arc::ptr_eq(&b.uploaded.as_ref().unwrap().1, &uploaded));
    assert!(b.pre.as_ref().is_some_and(|p| p.dehaze_stats.is_some()));
    let haze = b.haze.as_ref().expect("the reduced copy is kept");
    assert_eq!(haze.detail, edit.detail.options());
    let reduced = haze.reduced.clone();
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
    assert!(Arc::ptr_eq(&b.haze.as_ref().unwrap().reduced, &reduced));
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
    assert!(!Arc::ptr_eq(&b.haze.as_ref().unwrap().reduced, &reduced));
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
    let b = base.as_ref().unwrap();
    assert_eq!(b.haze.as_ref().unwrap().detail, None);
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
        assert!(b.pre.is_none() && b.haze.is_some());
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
    assert!(b.uploaded.is_some() && b.pre.is_none());
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
    assert!(b.uploaded.is_none() && b.pre.is_none());
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
                &mut None,
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
        // Two of a half float's ulps.
        assert!(
            worst < 2e-3,
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
                &mut None,
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
                           base: &mut Option<Base>,
                           gpu: &mut Option<greycard_gpu::Context>| {
                develop_job(
                    &Input::Raw(frame.clone()),
                    &edit,
                    0,
                    1,
                    base,
                    &mut None,
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
            assert!(b.uploaded.is_none() && b.pre.is_none() && b.haze.is_none());
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
        let mut develop = |edit: &Edit, base: &mut Option<Base>| {
            let (outcome, _) = develop_job(
                &Input::Raw(frame.clone()),
                edit,
                0,
                1,
                base,
                &mut None,
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
        let reduced = base
            .as_ref()
            .unwrap()
            .haze
            .as_ref()
            .unwrap()
            .reduced
            .clone();
        edit.detail.dehaze = 0.9;
        let moved = develop(&edit, &mut base);
        let kept = &base.as_ref().unwrap().haze.as_ref().unwrap().reduced;
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

#[test]
fn the_blend_is_linear_and_shares_at_the_ends() {
    let model = Arc::new(WorkingImage::from_data(2, 1, vec![1.0; 6]).unwrap());
    let plain = Arc::new(WorkingImage::from_data(2, 1, vec![0.0; 6]).unwrap());
    assert!(Arc::ptr_eq(&blend(&model, &plain, 1.0), &model));
    assert!(Arc::ptr_eq(&blend(&model, &plain, 0.0), &plain));
    assert!(Arc::ptr_eq(&blend(&model, &plain, 2.0), &model));
    let half = blend(&model, &plain, 0.25);
    assert!(half.data.iter().all(|v| (*v - 0.25).abs() < 1e-6));
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
            thumb_tag(file_stat(&raw).unwrap()),
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
    let stamp = thumb_tag(file_stat(&raw).unwrap()).stamp;
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
        tx.lock().unwrap().send(line).unwrap();
    });
    let mut thumbs = Thumbs::at(dir.join("thumbs"), greycard_library::thumbs::DEFAULT_CAP);
    let made = crate::grid::made_size(worker.pool.size());
    let kept = Thumb {
        width: 5,
        height: 4,
        rgb: vec![90; 60],
    };
    thumbs
        .put(&hash, made, thumb_tag(file_stat(&raw).unwrap()), &kept)
        .unwrap();
    worker.set_thumb_cache(Some(thumbs));
    std::fs::remove_file(&raw).unwrap();
    worker.send(Job::CachedThumbnail {
        index: 3,
        frame: Frame::file(raw.clone()),
        hash: hash.clone(),
        stamp,
    });
    let wait = std::time::Duration::from_secs(10);
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
    let (_, image) = develop_once(&frame, &mut base, &mut None);
    let image = image.unwrap();
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
        },
        ..greycard_edit::Adjustment::default()
    });
    let (_, left_out) = finish_export(
        image,
        &edit,
        base.as_ref(),
        &mut ai,
        &crate::export::Settings::default(),
    );
    let _ = std::fs::remove_dir_all(empty.parent().unwrap());
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
        kind: crate::finish::Source::SCENE,
        image: Developed::Halves(Arc::new(Halves {
            width: 1,
            height: 1,
            pixels: vec![half::f16::ZERO; 4],
        })),
        guide: Arc::new(crate::finish::Guide::NONE),
        planes: None,
        white: WhiteBase::IDENTITY,
        seconds: 0.0,
        detail: None,
        sharpen: None,
        dehaze: None,
        sources: Vec::new(),
        learned,
        fills,
    };
    assert!(left_out_of(&developed(LearnedReport::Off, FillReport::default()), None).is_empty());
    let model = greycard_ai::denoiser(greycard_edit::Learned::Fast.tier().unwrap()).unwrap();
    let said = left_out_of(
        &developed(
            LearnedReport::Missing(model),
            FillReport {
                missing: vec!["Patch 1".into()],
                ..FillReport::default()
            },
        ),
        None,
    );
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(said[0].contains(model.name), "{}", said[0]);
    assert!(said[1].starts_with("fill Patch 1:"), "{}", said[1]);
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
    let (mut base, mut learned) = (None, None);
    let mut develop = |edit: &Edit, base: &mut Option<Base>| {
        develop_job(
            &input,
            edit,
            0,
            1,
            base,
            &mut learned,
            &mut ai,
            None,
            None,
            &mut None,
            &deliver,
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
    let _ = std::fs::remove_dir_all(empty.parent().unwrap());
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
        base.as_ref().expect("a base").source,
        crate::finish::Source::Scene(crate::finish::Baseline::of_stops(b))
    );
    (frame.make, frame.model) = ("Canon".into(), "Canon EOS R5 Mark II".into());
    let mut base = None;
    develop_once(&Arc::new(frame), &mut base, &mut None);
    assert_eq!(base.expect("a base").source, crate::finish::Source::SCENE);
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
    let (mut base, mut learned) = (None, None);
    let first = develop_turned(&frame, &edit, 0, &mut base);
    let made = base.as_ref().expect("a base").image.clone();

    // Away to another file, whose base is made, and back.
    swap_base(
        &mut base,
        &mut learned,
        &mut shelf,
        Some(key.clone()),
        None,
        &edit,
        0,
        None,
    );
    assert!(base.is_none());
    develop_turned(&Arc::new(aberrated_frame(64, 48)), &edit, 0, &mut base);
    swap_base(
        &mut base,
        &mut learned,
        &mut shelf,
        None,
        Some(&key),
        &edit,
        0,
        None,
    );
    assert!(base.as_ref().is_some_and(|b| b.from_shelf));
    let again = develop_turned(&frame, &edit, 0, &mut base);
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
        &mut learned,
        &mut shelf,
        Some(key.clone()),
        Some(&key),
        &warmer,
        0,
        None,
    );
    assert!(base.is_none());
    develop_turned(&frame, &warmer, 0, &mut base);
    assert!(!Arc::ptr_eq(&base.as_ref().unwrap().image, &made));
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
    let (mut base, mut learned) = (None, None);
    let first = texture(develop_once(&frame, &mut base, &mut gpu).0);
    let b = base.as_ref().expect("a base");
    assert!(b.on_gpu && b.pre.is_some() && b.uploaded.is_some());
    let made = b.image.clone();
    drop(first);

    swap_base(
        &mut base,
        &mut learned,
        &mut shelf,
        Some(key.clone()),
        None,
        &edit,
        0,
        gpu.as_ref(),
    );
    develop_once(&Arc::new(aberrated_frame(200, 160)), &mut base, &mut gpu);
    swap_base(
        &mut base,
        &mut learned,
        &mut shelf,
        None,
        Some(&key),
        &edit,
        0,
        gpu.as_ref(),
    );
    let b = base.as_ref().expect("taken back");
    assert!(Arc::ptr_eq(&b.image, &made) && b.from_shelf);
    assert!(b.pre.is_none() && b.uploaded.is_none() && b.haze.is_none());

    let back = texture(develop_once(&frame, &mut base, &mut gpu).0);
    let b = base.as_ref().expect("the base");
    assert!(Arc::ptr_eq(&b.image, &made), "kept, not made again");
    assert!(b.pre.is_some() && b.uploaded.is_some(), "uploaded again");
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
        prefetch(&n, None, &mut shelf, None, None, &|| Ahead::Nothing),
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
                && l.contains(", finish ")
                && l.contains(", guide ")
                && l.contains("; the shelf: 1 base(s)")
        ),
        "{lines:?}"
    );
    assert!(matches!(
        prefetch(&n, None, &mut shelf, None, None, &|| Ahead::Nothing),
        Fetched::Skipped(why) if why == "its base is on the shelf"
    ));
    assert_eq!(shelf.len(), 1);

    // The open of it, as the worker's: the key from the file and its
    // decode, the base taken back, and the develop from it.
    let (input, _) = open(b).expect("our DNG decodes");
    let key = FileKey::of(b).expect("on the disk").decoded(&input);
    let (mut base, mut learned) = (None, None);
    swap_base(
        &mut base,
        &mut learned,
        &mut shelf,
        None,
        Some(&key),
        &edit,
        0,
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
    let develop = |base: &mut Option<Base>| {
        develop_job(
            &input,
            &edit,
            0,
            1,
            base,
            &mut None,
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
/// decode), the demosaic, the finish, and the lens step. The open of
/// the very frame it is making does not; that is said once, and the
/// base lands.
#[test]
fn a_request_stops_a_prefetch_between_its_stages() {
    use super::prefetch::{Ahead, Fetched, prefetch, said};
    use super::shelf::{Budget, Shelf};
    let (dir, paths) = dngs("prefetch-stop", &["pf-stop-B.dng"], 96, 64);
    let n = neighbor_at(&paths[0], Edit::default(), Side::Previous);
    for (asked, stage) in [(1, "prepare"), (2, "demosaic"), (3, "finish"), (4, "lens")] {
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        let calls = std::cell::Cell::new(0);
        let ahead = || {
            calls.set(calls.get() + 1);
            if calls.get() >= asked {
                Ahead::Work
            } else {
                Ahead::Nothing
            }
        };
        assert_eq!(
            prefetch(&n, None, &mut shelf, None, None, &ahead),
            Fetched::Abandoned(stage)
        );
        assert_eq!(shelf.len(), 0, "nothing kept when stopped before {stage}");
        assert_eq!(calls.get(), asked, "asked once a stage");
    }
    assert!(said("pf-stop-B.dng").iter().any(|l| l
        == "the prefetch of the previous frame, pf-stop-B.dng, abandoned before its demosaic: \
            the worker was asked for something"));

    let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
    assert_eq!(
        prefetch(&n, None, &mut shelf, None, None, &|| Ahead::ItsOpen),
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
            &mut shelf,
            None,
            None,
            &|| Ahead::Nothing
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
            &mut shelf,
            None,
            None,
            &|| Ahead::Nothing
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
    };
    let wait = std::time::Duration::from_secs(60);
    worker.send(open(&paths[0], 1));
    worker.neighbors(
        1,
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

/// A worker that opens no frame (an export's from the command line)
/// prefetches nothing, even when handed neighbors.
#[test]
fn a_worker_that_opens_nothing_prefetches_nothing() {
    use super::prefetch::{QUIET, said};
    let (dir, paths) = dngs("prefetch-none", &["pf-none-B.dng"], 64, 48);
    let worker = Worker::new(|_| {});
    worker.neighbors(1, vec![neighbor_at(&paths[0], Edit::default(), Side::Next)]);
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
    worker.neighbors(1, vec![neighbor_at(&p[1], Edit::default(), Side::Next)]);
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
        vec![
            neighbor_at(&p[2], Edit::default(), Side::Next),
            neighbor_at(&p[3], Edit::default(), Side::Previous),
        ],
    );
    assert_eq!(at.recv_timeout(wait).expect("held"), "prepare");
    open(&p[4], 3);
    worker.neighbors(3, vec![neighbor_at(&p[3], Edit::default(), Side::Next)]);
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
    assert!(!worker.neighbors_again(7, &paths[0], b(), "a test"));
    assert!(worker.queue.0.lock().unwrap().neighbors.is_none());
    worker.send(Job::Open {
        path: paths[0].clone(),
        version: None,
        edit: Edit::default(),
        generation: 7,
        seed_blend: false,
        turn: 0,
        stance_known: false,
    });
    let wait = std::time::Duration::from_secs(60);
    assert_eq!(rx.recv_timeout(wait).expect("the open lands"), 7);
    assert_eq!(
        worker.queue.0.lock().unwrap().landed,
        Some((7, paths[0].clone()))
    );
    // Away to the grid: let go. Back on another file: not taken.
    worker.set_open_frame(None);
    assert!(!worker.neighbors_again(7, std::path::Path::new("/nowhere/other.CR3"), b(), "a test"));
    assert!(worker.queue.0.lock().unwrap().neighbors.is_none());
    // Back on A: taken under A's open.
    worker.set_open_frame(Some(Frame::file(paths[0].clone())));
    assert!(worker.neighbors_again(7, &paths[0], b(), "a test"));
    {
        let q = worker.queue.0.lock().unwrap();
        let n = q.neighbors.as_ref().expect("sent again");
        assert_eq!(n.generation, 7);
    }
    // Sent again for the same open (the filter or the order moved):
    // its list, waiting or done, is replaced.
    assert!(worker.neighbors_again(7, &paths[0], Vec::new(), "a test"));
    // A list for another open waiting already is not replaced.
    worker.neighbors(9, b());
    assert!(!worker.neighbors_again(7, &paths[0], b(), "a test"));
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
    };
    worker.set_open_frame(Some(Frame::file(paths[0].clone())));
    worker.send(open(&paths[0], Edit::default(), 1));
    worker.neighbors(1, vec![neighbor_at(&paths[1], Edit::default(), Side::Next)]);
    assert_eq!(rx.recv_timeout(wait).expect("the open lands"), 1);
    until("B prefetched", &|| count("prefetched pf-ed-B.dng") == 1);

    // After the base: kept, and not made again.
    let mut brighter = Edit::default();
    brighter.light.exposure = 1.0;
    worker.edit_changed(paths[1].clone(), Some(holds(&brighter)));
    assert!(worker.neighbors_again(
        1,
        &paths[0],
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
    worker.neighbors(1, vec![neighbor_at(&paths[1], Edit::default(), Side::Next)]);
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
        vec![neighbor_at(&paths[1], warmer.clone(), Side::Next)],
        "a neighbor's edit changed"
    ));
    assert_eq!(at.recv_timeout(wait).expect("held"), "prepare");
    worker.edit_changed(paths[1].clone(), Some(holds(&Edit::default())));
    assert!(worker.neighbors_again(
        1,
        &paths[0],
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
