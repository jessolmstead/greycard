use super::*;

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
fn aberrated_frame(width: usize, height: usize) -> RawFrame {
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
            source: crate::finish::Source::Scene,
            stamp: 1,
            patched: None,
            pre: None,
            uploaded: None,
            haze: None,
            planes: None,
            ca_on_gpu: false,
            stand_in: None,
            profile: None,
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
        source: crate::finish::Source::Scene,
        stamp: 1,
        patched: None,
        pre: None,
        uploaded: None,
        haze: None,
        planes: None,
        ca_on_gpu: false,
        stand_in: None,
        profile: None,
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
fn skipped(what: &str) {
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
    assert!(base.as_ref().is_some_and(|b| b.ca_on_gpu));
    let stamp = base.as_ref().unwrap().stamp;
    // The export: no GPU, the same edit.
    let (outcome, exported) = develop_once(&frame, &mut base, &mut None);
    assert!(matches!(outcome, Outcome::Developed { .. }));
    let b = base.as_ref().unwrap();
    assert!(
        !b.ca_on_gpu && b.stamp == stamp + 1,
        "a fresh base on the CPU"
    );
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
        source: crate::finish::Source::Scene,
        stamp: 0,
        patched: None,
        pre: None,
        uploaded: None,
        haze: None,
        planes: None,
        ca_on_gpu: false,
        stand_in: None,
        profile: None,
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
    base.as_mut().unwrap().ca_on_gpu = true;
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
        last.as_ref().is_some_and(|l| !l.ca_on_gpu),
        "the last develop is the CPU's now"
    );
    assert!(base.as_ref().is_some_and(|b| !b.ca_on_gpu));
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
    assert!(base.as_ref().is_some_and(|b| !b.ca_on_gpu));
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
    assert!(base.as_ref().is_some_and(|b| !b.ca_on_gpu));
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

/// What keeping the open frame's edit costs, on the raws of the
/// folder `GREYCARD_SAMPLES` names (ignored; run it in release with
/// `--ignored --nocapture`): each raw developed once on the CPU under
/// a brightening, then its pictures made as a save makes them, from
/// the CPU's picture (reduced, straightened, fitted, finished,
/// shrunk, encoded and kept) and, when there is a GPU, the same
/// picture read back from a viewport texture a whole factor smaller.
#[test]
#[ignore]
fn the_edit_s_pictures_of_the_samples() {
    let Some(dir) = std::env::var_os("GREYCARD_SAMPLES") else {
        eprintln!("GREYCARD_SAMPLES names no folder; nothing measured");
        return;
    };
    let mut raws: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| greycard_core::decode::is_raw_path(p))
        .collect();
    raws.sort();
    let gpu = greycard_gpu::Context::own()
        .inspect_err(|e| eprintln!("no GPU for the read back: {e}"))
        .ok();
    let scratch = thumb_scratch("edit-samples");
    for path in raws {
        let (input, _) = timed_open(&path).unwrap();
        let mut edit = Edit::default();
        edit.light.exposure = 1.0;
        let mut base = None;
        let deliver: Deliver = Arc::new(|_| {});
        let (_, image) = develop_job(
            &input,
            &edit,
            0,
            1,
            &mut base,
            &mut None,
            &mut Ai::new(),
            None,
            None,
            &mut None,
            &deliver,
        );
        let image = image.expect("a picture on the CPU");
        let b = base.as_ref().unwrap();
        let source = (image.width as u32, image.height as u32);
        let factor = crate::edited::factor_for(source, &edit.geometry);
        let finish = || crate::edited::Finish {
            edit: edit.clone(),
            turn: 0,
            source,
            full: source,
            rasters: Default::default(),
            clip_level: b.clip_level,
            guide: Some(b.guide.clone()),
            kind: b.source,
            white: b.white_shift(),
            detail: None,
        };
        let cache: ThumbCache = Arc::new(Mutex::new(Some(
            Thumbs::at(
                scratch.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(2048, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        )));
        let edited = crate::edited::Edited::new(cache);
        let hash = greycard_library::hash_file(&path).unwrap();
        let cpu = Instant::now();
        crate::edited::keep(
            &edited,
            crate::edited::Keeping {
                frame: Frame::file(path.clone()),
                hash: hash.clone(),
                key: 1,
                developed: crate::edited::Developed::Full(image.clone()),
                factor,
                read_back: None,
                marked: edited.mark(&hash, 1, crate::edited::EDITED_RECIPE).unwrap(),
                iso: None,
                finish: finish(),
            },
            |_| {},
        );
        let cpu = cpu.elapsed().as_secs_f64();
        let read_back = gpu.as_ref().map(|ctx| {
            let halves = Halves::from_image(&image, None);
            let texture = ctx.viewport_texture(source.0, source.1);
            ctx.queue().write_texture(
                greycard_gpu::wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: greycard_gpu::wgpu::Origin3d::ZERO,
                    aspect: greycard_gpu::wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&halves.pixels),
                greycard_gpu::wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(source.0 * 8),
                    rows_per_image: Some(source.1),
                },
                greycard_gpu::wgpu::Extent3d {
                    width: source.0,
                    height: source.1,
                    depth_or_array_layers: 1,
                },
            );
            ctx.queue().submit([]);
            let started = Instant::now();
            let reduced = crate::edited::read_back_reduced(ctx, &texture, factor).unwrap();
            let read = started.elapsed().as_secs_f64();
            let started = Instant::now();
            crate::edited::keep(
                &edited,
                crate::edited::Keeping {
                    frame: Frame::file(path.clone()),
                    hash: hash.clone(),
                    key: 2,
                    developed: crate::edited::Developed::Reduced(reduced),
                    factor,
                    read_back: Some(read),
                    marked: edited.mark(&hash, 2, crate::edited::EDITED_RECIPE).unwrap(),
                    iso: None,
                    finish: finish(),
                },
                |_| {},
            );
            (read, started.elapsed().as_secs_f64())
        });
        println!(
            "{}: {}x{} ({:.1} MP), factor {factor}: from the CPU's picture {:.0} ms{}",
            path.file_name().unwrap().to_string_lossy(),
            source.0,
            source.1,
            source.0 as f64 * source.1 as f64 / 1e6,
            cpu * 1e3,
            read_back
                .map(|(r, k)| format!(
                    "; from the GPU's, read back {:.0} ms and made {:.0} ms",
                    r * 1e3,
                    k * 1e3
                ))
                .unwrap_or_default(),
        );
    }
    crate::testing::remove_dir_retry(&scratch);
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

/// The Sky shape over real frames, the product's own path: each
/// file opened and developed as the worker does under the default
/// edit, then the Sky raster made by `Ai::raster` twice, once on
/// the providers this machine offers (WebGPU where there is one)
/// and once on the CPU alone, and the two compared. Set
/// `GREYCARD_SAMPLES` to a folder of frames (copies: nothing is
/// written beside them) and `GREYCARD_MODELS` to a store holding
/// the Sky model and SAM, and run with `--ignored`; without both
/// it passes at once. `GREYCARD_SKY_OUT` names a folder for each
/// frame's preview (`<stem>.jpg`), its Sky raster (`<stem>-sky.png`)
/// and `report.txt`; `GREYCARD_SKY_ONLY` a comma list of stems.
/// Fails if any frame with no sky gets a single pixel of sky: the
/// stems in `GREYCARD_SKY_NONE` (a comma list), else the five in the
/// user's set. A frame that will not open or develop is passed over
/// with a line.
#[test]
#[ignore]
fn the_sky_over_real_frames() {
    use greycard_ai::Provider;
    let (Some(samples), Some(models)) = (
        std::env::var_os("GREYCARD_SAMPLES"),
        std::env::var_os("GREYCARD_MODELS"),
    ) else {
        return;
    };
    let out = std::env::var_os("GREYCARD_SKY_OUT").map(PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out).unwrap();
    }
    let only: Option<Vec<String>> = std::env::var("GREYCARD_SKY_ONLY")
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect());
    let no_sky: Vec<String> = std::env::var("GREYCARD_SKY_NONE").map_or_else(
        |_| {
            ["5M0A5135", "DSCF0153", "DSCF0186", "5M0A0957", "5M0A0952"]
                .map(str::to_string)
                .to_vec()
        },
        |s| s.split(',').map(str::to_string).collect(),
    );
    let store = greycard_ai::Store::at(models);
    let mut develop_ai = Ai::for_test(store.clone(), vec![Provider::Cpu]);
    let mut near = Ai::for_test(store.clone(), Provider::available());
    let mut cpu = Ai::for_test(store, vec![Provider::Cpu]);
    let mut frames: Vec<PathBuf> = std::fs::read_dir(&samples)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| greycard_core::decode::is_raw_path(p))
        .collect();
    frames.sort();
    let mut lines = Vec::new();
    let mut painted = Vec::new();
    let shape = Shape::Sky { picks: Vec::new() };
    let deliver: Deliver = Arc::new(|_| {});
    for path in frames {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        if only.as_ref().is_some_and(|o| !o.contains(&stem)) {
            continue;
        }
        let input = match open(&path) {
            Ok((input, _)) => input,
            Err(e) => {
                let line = format!("{stem}: passed over, it will not open: {e:#}");
                println!("{line}");
                lines.push(line);
                continue;
            }
        };
        let mut base = None;
        let t = Instant::now();
        let _ = develop_job(
            &input,
            &Edit::default(),
            0,
            1,
            &mut base,
            &mut None,
            &mut develop_ai,
            None,
            None,
            &mut None,
            &deliver,
        );
        let develop = t.elapsed().as_secs_f64();
        let Some(b) = base.as_ref() else {
            let line = format!("{stem}: passed over, it did not develop");
            println!("{line}");
            lines.push(line);
            continue;
        };
        let run = |ai: &mut Ai| {
            ai.forget(None);
            let t = Instant::now();
            let made = ai
                .raster(
                    b.stamp,
                    &b.image,
                    &b.edit,
                    b.source,
                    b.turn,
                    (1, 0),
                    &shape,
                    None,
                )
                .unwrap_or_else(|e| panic!("{stem}: {e}"));
            let total = t.elapsed().as_secs_f64();
            (made, total, ai.last_sky.clone().expect("a sky report"))
        };
        let (made, total, r) = run(&mut near);
        let (cpu_made, cpu_total, c) = run(&mut cpu);
        let data = made.raster.data();
        let cpu_data = cpu_made.raster.data();
        let n = data.len() as f64;
        let area = data.iter().filter(|&&v| v > 127).count() as f64 / n;
        let (mut max, mut sum, mut flips) = (0u8, 0u64, 0usize);
        for (a, c) in data.iter().zip(cpu_data) {
            max = max.max(a.abs_diff(*c));
            sum += a.abs_diff(*c) as u64;
            flips += ((*a > 127) != (*c > 127)) as usize;
        }
        if no_sky.contains(&stem) && data.iter().any(|&v| v > 0) {
            painted.push(stem.clone());
        }
        let stages = |r: &crate::ai::SkyReport, total: f64| {
            let s = r.stages;
            let known = r.load + r.prior + s.gate + s.seeds + s.outline + s.edge + r.raster;
            format!(
                "total {total:.2}s = preview {:.2} + prior {:.3} on {} + gate {:.3} + seeds {:.3} \
                 + outline {:.3} (SAM load {:.2}, embed {:.3} on {}, {} decodes) + edge {:.3} \
                 + raster {:.3}",
                total - known,
                r.prior,
                r.prior_on.map_or("-", |p| p.name()),
                s.gate,
                s.seeds,
                s.outline,
                r.sam_load,
                r.embed,
                r.sam_on.map_or("-", |p| p.name()),
                r.decodes,
                s.edge,
                r.raster,
            )
        };
        // The matte made from the frame brought down to the
        // preview's size instead of at its own, for the time.
        let at_preview = match (near.sky_prior(), &r.outline) {
            (Some(prior), Some(outline)) => {
                let (w, h) = (prior.width, prior.height);
                let mut small = WorkingImage::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        let (sx, sy) = (x * b.image.width / w, y * b.image.height / h);
                        let from = (sy * b.image.width + sx) * 3;
                        small.data[(y * w + x) * 3..][..3]
                            .copy_from_slice(&b.image.data[from..from + 3]);
                    }
                }
                let t = Instant::now();
                let _ = greycard_ai::matte::color_line(outline, prior, &small, &[], w, h);
                format!("{:.3}s at {w}x{h}", t.elapsed().as_secs_f64())
            }
            _ => "-".to_string(),
        };
        let line = format!(
            "{stem}: {} {:.1}% seeded {} | develop {develop:.1}s | {} | CPU: {} | \
             against the CPU: max {max}/255, mean {:.2e}, {:.4}% across a half | \
             matte at the preview's size {at_preview}, at the frame's {}x{}",
            match r.no_sky {
                Some(why) => format!("no sky ({why})"),
                None => "sky".to_string(),
            },
            area * 100.0,
            r.seeded,
            stages(&r, total),
            stages(&c, cpu_total),
            sum as f64 / n / 255.0,
            flips as f64 / n * 100.0,
            b.image.width,
            b.image.height,
        );
        println!("{line}");
        lines.push(line);
        if let Some(out) = &out {
            let rgb = crate::ai::preview(&b.image, &b.edit, b.source);
            image::save_buffer(
                out.join(format!("{stem}.jpg")),
                &rgb.data,
                rgb.width as u32,
                rgb.height as u32,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
            // The trimap the matte started from: sky white, in
            // question grey, not sky black; and the prior's labels.
            if let (Some(prior), Some(outline)) = (near.sky_prior(), &r.outline) {
                let map: Vec<u8> = greycard_ai::matte::trimap(outline, prior)
                    .into_iter()
                    .map(|k| match k {
                        greycard_ai::matte::Known::Sky => 255,
                        greycard_ai::matte::Known::Unknown => 128,
                        greycard_ai::matte::Known::NotSky => 0,
                    })
                    .collect();
                for (name, data) in [("trimap", &map), ("labels", &prior.labels)] {
                    image::save_buffer(
                        out.join(format!("{stem}-{name}.png")),
                        data,
                        prior.width as u32,
                        prior.height as u32,
                        image::ExtendedColorType::L8,
                    )
                    .unwrap();
                }
            }
            image::save_buffer(
                out.join(format!("{stem}-sky.png")),
                data,
                made.raster.width as u32,
                made.raster.height as u32,
                image::ExtendedColorType::L8,
            )
            .unwrap();
        }
    }
    if let Some(out) = &out {
        std::fs::write(out.join("report.txt"), lines.join("\n") + "\n").unwrap();
    }
    assert!(
        painted.is_empty(),
        "sky painted on frames with none: {painted:?}"
    );
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
    assert!(matches!(
        &first,
        Outcome::Developed {
            learned: LearnedReport::Missing(_),
            ..
        }
    ));
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
    assert!(said[0].starts_with("the learned denoiser:"), "{}", said[0]);
}

/// The People mask on real frames, through the editor's own path:
/// each frame of `GREYCARD_PARTS_PICTURES` (the trial's JPEGs)
/// opened and developed, its people found, and the parts made on
/// it, with the models of `GREYCARD_MODELS`.
/// - On a portrait an Iris is two small pieces, and a part of the
///   one woman on it, as the panel keeps her, finds her there.
/// - Hers pasted onto her other frame finds her; a group's woman
///   pasted onto the portrait is never decided alone.
/// - On the group, each one's Top resolves alone on her own picture
///   and is hers: centered under her face, and no two overlapping.
/// - A person picked there is still that person after the picture
///   is turned or brightened, and her signature does not move.
/// - A person pasted to a picture of two or more people asks.
/// - With a Subject model in the store, a Whole person's edge is
///   as fine as Subject's, and on the group no two share more
///   than with SAM's edge alone.
///
/// Prints each part's seconds and provider; with
/// `GREYCARD_PARTS_OUT` set, writes each mask there as a PNG.
#[test]
#[ignore]
fn parts_on_real_frames() {
    use crate::ai::Unresolved;
    use greycard_ai::Provider;
    use greycard_edit::mask::{PARTS, Person, Turned};
    let Some(models) = std::env::var_os("GREYCARD_MODELS") else {
        return;
    };
    let dir = std::env::var_os("GREYCARD_PARTS_PICTURES")
        .map(PathBuf::from)
        .expect("GREYCARD_PARTS_PICTURES: the trial's JPEGs, with GREYCARD_MODELS set");
    let out = std::env::var_os("GREYCARD_PARTS_OUT").map(PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out).unwrap();
    }
    let store = greycard_ai::Store::at(models);
    // Whole person's edge is Subject's where the store has it.
    let edge = store.have(&greycard_ai::SUBJECT_WEBGPU) || store.have(&greycard_ai::SUBJECT);
    let mut develop_ai = Ai::for_test(store.clone(), vec![Provider::Cpu]);
    let mut ai = Ai::for_test(store, Provider::available());
    let deliver: Deliver = Arc::new(|_| {});
    // A frame developed under `edit` at `turn`, and the Parts side
    // told it is that file, as opening it tells the editor's.
    let mut develop = |stem: &str, edit: &Edit, turn: u8, ai: &mut Ai| {
        let path = dir.join(format!("{stem}.jpg"));
        let (input, _) = open(&path).unwrap_or_else(|e| panic!("{stem}: {e:#}"));
        let mut base = None;
        let _ = develop_job(
            &input,
            edit,
            turn,
            1,
            &mut base,
            &mut None,
            &mut develop_ai,
            None,
            None,
            &mut None,
            &deliver,
        );
        ai.forget(Some(path));
        base.unwrap_or_else(|| panic!("{stem} did not develop"))
    };
    let save = |name: &str, raster: &Raster| {
        if let Some(out) = &out {
            let height = raster.data().len() / greycard_edit::brush::RASTER_WIDTH;
            image::save_buffer(
                out.join(format!("{name}.png")),
                raster.data(),
                greycard_edit::brush::RASTER_WIDTH as u32,
                height as u32,
                image::ExtendedColorType::L8,
            )
            .unwrap();
        }
    };
    // Each frame's people as a shape stores them, for a sidecar
    // written by hand to look at.
    let dump = |stem: &str, people: &[crate::ai::Candidate]| {
        if let Some(out) = &out {
            let people: Vec<Person> = people.iter().map(|p| p.person()).collect();
            std::fs::write(
                out.join(format!("people-{stem}.json")),
                serde_json::to_string(&people).unwrap(),
            )
            .unwrap();
        }
    };
    let on = |r: &Raster| r.data().iter().filter(|&&v| v > 127).count();
    let share = |r: &Raster| on(r) as f64 / r.data().len() as f64;
    // A mask's soft band: its pixels between a tenth and nine
    // tenths for each pixel of its edge (over a half, a neighbor
    // under it).
    let band = |r: &Raster| {
        let (d, w) = (r.data(), greycard_edit::brush::RASTER_WIDTH);
        let h = d.len() / w;
        let soft = d.iter().filter(|&&v| v > 25 && v < 230).count();
        let mut edge = 0usize;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let under = |j: usize| d[j] <= 127;
                if d[i] > 127
                    && ((x > 0 && under(i - 1))
                        || (x + 1 < w && under(i + 1))
                        || (y > 0 && under(i - w))
                        || (y + 1 < h && under(i + w)))
                {
                    edge += 1;
                }
            }
        }
        soft as f64 / edge.max(1) as f64
    };
    let part = |label: &str, person: Option<Person>| {
        let p = PARTS.iter().find(|p| p.label == label).unwrap();
        Shape::Part {
            phrase: p.phrase.to_string(),
            route: p.route.clone(),
            person,
        }
    };
    let plain = Edit::default();

    // A portrait: every part of everyone, timed; the first run
    // pays for the load and the encoding.
    let b = develop("DSCF0835", &plain, 0, &mut ai);
    let (people, provider, seconds) = ai.people(b.stamp, &b.image, b.source, b.turn).unwrap();
    println!(
        "DSCF0835: {} people on {} in {seconds:.2}s (load and encode)",
        people.len(),
        provider.name()
    );
    assert_eq!(people.len(), 1, "one woman");
    dump("DSCF0835", &people);
    for (i, p) in PARTS.iter().enumerate() {
        let shape = part(p.label, None);
        let made = ai
            .raster(
                b.stamp,
                &b.image,
                &b.edit,
                b.source,
                b.turn,
                (1, i),
                &shape,
                None,
            )
            .unwrap_or_else(|e| panic!("{}: {e}", p.label));
        assert!(made.part.is_none());
        println!(
            "  {:<12} {:>6.2}s on {:<6} {:>6.2}% of the frame",
            p.label,
            made.seconds,
            made.provider.map_or("-", |p| p.name()),
            100.0 * share(&made.raster)
        );
        save(&format!("DSCF0835-{}", p.label), &made.raster);
        if p.label == "Iris" {
            let s = share(&made.raster);
            assert!(s > 0.0 && s < 0.002, "two irises, small: {s}");
        }
    }
    // A part chosen on the portrait is hers, the one person on it
    // kept as a pick is, and on her own picture it finds her.
    let her = people[0].person();
    assert!(her.picture.is_some());
    let made = ai
        .raster(
            b.stamp,
            &b.image,
            &b.edit,
            b.source,
            b.turn,
            (1, PARTS.len()),
            &part("Lips", Some(her.clone())),
            None,
        )
        .unwrap();
    assert!(made.part.is_none(), "hers on her own picture");
    assert!(share(&made.raster) > 0.0, "her lips");
    println!(
        "  Lips, hers: {:.2}s, {:.3}% of the frame",
        made.seconds,
        100.0 * share(&made.raster)
    );
    // Her whole, as the menu's first entry makes it on a portrait:
    // hers, and on a picture of one, all but everyone's.
    let everyone = ai
        .raster(
            b.stamp,
            &b.image,
            &b.edit,
            b.source,
            b.turn,
            (1, 0),
            &part("Whole person", None),
            None,
        )
        .unwrap()
        .raster;
    let made = ai
        .raster(
            b.stamp,
            &b.image,
            &b.edit,
            b.source,
            b.turn,
            (1, PARTS.len() + 1),
            &part("Whole person", Some(her.clone())),
            None,
        )
        .unwrap();
    assert!(made.part.is_none(), "her whole on her own picture");
    save("DSCF0835-person-hers", &made.raster);
    let both = made
        .raster
        .data()
        .iter()
        .zip(everyone.data())
        .filter(|&(&a, &b)| a > 127 && b > 127)
        .count();
    println!(
        "  Whole person, hers: {:.2}s, {:.2}% of the frame, {both} of everyone's {}",
        made.seconds,
        100.0 * share(&made.raster),
        on(&everyone)
    );
    assert!(share(&made.raster) > 0.05, "her, whole");
    assert!(both * 10 > on(&everyone) * 9, "the one person is everyone");
    // Her edge is Subject's: its soft band per edge pixel as
    // narrow as Subject's own, within a sixth (SAM's alone ran 1.3
    // times as wide), and Subject's raster the same run's. Without
    // Subject in the store, SAM's edge, and the status line says so.
    if edge {
        let subject = ai
            .raster(
                b.stamp,
                &b.image,
                &b.edit,
                b.source,
                b.turn,
                (1, PARTS.len() + 2),
                &Shape::Subject {},
                None,
            )
            .unwrap();
        assert_eq!(subject.seconds, 0.0, "Subject's matte, kept from her edge");
        let (hers, theirs) = (band(&made.raster), band(&subject.raster));
        println!("  Whole person's band {hers:.1}, Subject's {theirs:.1}");
        assert!(
            hers < 1.15 * theirs,
            "her band {hers:.1} against {theirs:.1}"
        );
    } else {
        println!(
            "  Whole person's band {:.1}, no Subject",
            band(&made.raster)
        );
        let note = made.note.as_deref().unwrap_or_default();
        assert!(note.contains("Subject model is not downloaded"), "{note}");
    }

    // The group: each person's Top on her own picture is decided
    // alone, and hers.
    let g = develop("5M0A4169", &plain, 0, &mut ai);
    let (group, _, seconds) = ai.people(g.stamp, &g.image, g.source, g.turn).unwrap();
    println!("5M0A4169: {} people in {seconds:.2}s", group.len());
    assert_eq!(group.len(), 5, "the group frame");
    assert!(group.iter().all(|p| p.picture.is_some()));
    dump("5M0A4169", &group);
    let w = greycard_edit::brush::RASTER_WIDTH;
    let aspect = g.image.height as f32 / g.image.width as f32;
    let mut tops = Vec::new();
    for (i, p) in group.iter().enumerate() {
        let shape = part("Top", Some(p.person()));
        let made = ai
            .raster(
                g.stamp,
                &g.image,
                &g.edit,
                g.source,
                g.turn,
                (2, i),
                &shape,
                None,
            )
            .unwrap();
        assert!(made.part.is_none(), "#{i} resolves on her own picture");
        save(&format!("5M0A4169-top-{i}"), &made.raster);
        // Her top's middle, across, is nearer her face than anyone
        // else's.
        let r = made.raster.data();
        let (mut sx, mut n) = (0.0f64, 0usize);
        for (k, &v) in r.iter().enumerate() {
            if v > 127 {
                sx += (k % w) as f64;
                n += 1;
            }
        }
        assert!(n > 0, "#{i} has a top");
        let mid = (sx / n as f64 / w as f64) as f32;
        let nearest = (0..group.len())
            .min_by(|&a, &b| {
                (group[a].at[0] - mid)
                    .abs()
                    .total_cmp(&(group[b].at[0] - mid).abs())
            })
            .unwrap();
        println!(
            "  Top of #{i}: {:.2}s, {:.2}% of the frame, nearest face #{nearest}",
            made.seconds,
            100.0 * share(&made.raster)
        );
        assert_eq!(nearest, i, "#{i}'s top is hers");
        tops.push(made.raster);
    }
    // No two the same, and none taking a piece of another's.
    for i in 0..tops.len() {
        for j in i + 1..tops.len() {
            let both = tops[i]
                .data()
                .iter()
                .zip(tops[j].data())
                .filter(|&(&a, &b)| a > 127 && b > 127)
                .count();
            let least = on(&tops[i]).min(on(&tops[j]));
            println!("  #{i} and #{j} overlap {both} of {least}");
            assert!(both * 50 < least, "#{i} and #{j} overlap {both} of {least}");
        }
    }

    // Each one whole, on her own picture: decided alone, over her
    // own face, and no two sharing much; All people holds them all.
    let mut wholes = Vec::new();
    for (i, p) in group.iter().enumerate() {
        let made = ai
            .raster(
                g.stamp,
                &g.image,
                &g.edit,
                g.source,
                g.turn,
                (9, i),
                &part("Whole person", Some(p.person())),
                None,
            )
            .unwrap();
        assert!(
            made.part.is_none(),
            "#{i} whole resolves on her own picture"
        );
        save(&format!("5M0A4169-person-{i}"), &made.raster);
        let r = made.raster.data();
        let h = r.len() / w;
        let [cx, cy] = [
            (p.face[0] + p.face[2]) / 2.0,
            (p.face[1] + p.face[3]) / 2.0 / aspect,
        ];
        let at = (cy * h as f32) as usize * w + (cx * w as f32) as usize;
        println!(
            "  Whole person #{i}: {:.2}s, {:.2}% of the frame, body {}, {} at her face",
            made.seconds,
            100.0 * share(&made.raster),
            p.body.is_some(),
            r[at]
        );
        assert!(r[at] > 127, "#{i} whole holds her face");
        wholes.push(made.raster);
    }
    // Each one's edge as fine as Subject's, within a sixth: SAM's
    // alone ran to 1.26 times as wide on these five.
    if edge {
        let subject = ai
            .raster(
                g.stamp,
                &g.image,
                &g.edit,
                g.source,
                g.turn,
                (9, group.len() + 1),
                &Shape::Subject {},
                None,
            )
            .unwrap();
        let theirs = band(&subject.raster);
        for (i, one) in wholes.iter().enumerate() {
            let hers = band(one);
            println!("  whole #{i}'s band {hers:.1}, Subject's {theirs:.1}");
            assert!(
                hers < 1.15 * theirs,
                "#{i}'s band {hers:.1} against {theirs:.1}"
            );
        }
    } else {
        for (i, one) in wholes.iter().enumerate() {
            println!("  whole #{i}'s band {:.1}, no Subject", band(one));
        }
    }
    // The same five with SAM's edge alone, under other keys.
    ai.sams_edge_only = true;
    let sams: Vec<Arc<Raster>> = group
        .iter()
        .enumerate()
        .map(|(i, p)| {
            ai.raster(
                g.stamp,
                &g.image,
                &g.edit,
                g.source,
                g.turn,
                (10, i),
                &part("Whole person", Some(p.person())),
                None,
            )
            .unwrap()
            .raster
        })
        .collect();
    ai.sams_edge_only = false;
    let overlap = |a: &Raster, b: &Raster| {
        a.data()
            .iter()
            .zip(b.data())
            .filter(|&(&a, &b)| a > 127 && b > 127)
            .count()
    };
    // No two share more than SAM's edges alone did, and not much.
    for i in 0..wholes.len() {
        for j in i + 1..wholes.len() {
            let both = overlap(&wholes[i], &wholes[j]);
            let before = overlap(&sams[i], &sams[j]);
            let least = on(&wholes[i]).min(on(&wholes[j]));
            println!("  whole #{i} and #{j} overlap {both} of {least}, SAM's {before}");
            assert!(
                both <= before && both * 20 < least,
                "whole #{i} and #{j} overlap {both} of {least}, SAM's {before}"
            );
        }
    }
    let made = ai
        .raster(
            g.stamp,
            &g.image,
            &g.edit,
            g.source,
            g.turn,
            (9, group.len()),
            &part("Whole person", None),
            None,
        )
        .unwrap();
    save("5M0A4169-person-all", &made.raster);
    for (i, one) in wholes.iter().enumerate() {
        let held = one
            .data()
            .iter()
            .zip(made.raster.data())
            .filter(|&(&a, &b)| a > 127 && b > 127)
            .count();
        println!("  All people holds {held} of #{i}'s {}", on(one));
        assert!(held * 10 > on(one) * 9, "All people holds #{i}");
    }

    // One of them after a global edit and a turn: still her, decided
    // on her own picture by where her face was, and her signature
    // the same under the brighter edit.
    let k = 2;
    let first = group[k].person();
    let mut bright = Edit::default();
    bright.light.exposure = 0.7;
    let e = develop("5M0A4169", &bright, 0, &mut ai);
    let (again, _, _) = ai.people(e.stamp, &e.image, e.source, e.turn).unwrap();
    for (a, b) in again.iter().zip(&group) {
        let most = a
            .signature
            .values
            .iter()
            .zip(&b.signature.values)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max);
        assert!(most <= 1e-3, "a signature moved by {most} under +0.7 EV");
    }
    let made = ai
        .raster(
            e.stamp,
            &e.image,
            &bright,
            e.source,
            e.turn,
            (7, 0),
            &part("Top", Some(first.clone())),
            None,
        )
        .unwrap();
    assert!(made.part.is_none(), "brighter: still decided");
    let both = made
        .raster
        .data()
        .iter()
        .zip(tops[k].data())
        .filter(|&(&a, &b)| a > 127 && b > 127)
        .count();
    assert!(both * 10 > on(&tops[k]) * 9, "brighter: the same top");
    println!("+0.7 EV: #{k} decided alone, the same top");

    let t = develop("5M0A4169", &plain, 1, &mut ai);
    let mut turned = part("Top", Some(first));
    turned.turn(Turned::new(1, 1.0 / aspect));
    let made = ai
        .raster(
            t.stamp,
            &t.image,
            &t.edit,
            t.source,
            t.turn,
            (8, 0),
            &turned,
            None,
        )
        .unwrap();
    assert!(made.part.is_none(), "turned: still decided");
    save("5M0A4169-top-turned", &made.raster);
    // Her top, turned: the same cells, a quarter clockwise.
    let tw = w;
    let th = made.raster.data().len() / tw;
    let (gw, gh) = (w, tops[k].data().len() / w);
    let mut hit = 0usize;
    for (idx, &v) in tops[k].data().iter().enumerate() {
        if v > 127 {
            let (x, y) = (idx % gw, idx / gw);
            let (nx, ny) = (
                ((1.0 - (y as f32 + 0.5) / gh as f32) * tw as f32) as usize,
                (((x as f32 + 0.5) / gw as f32) * th as f32) as usize,
            );
            if made.raster.data()[ny.min(th - 1) * tw + nx.min(tw - 1)] > 127 {
                hit += 1;
            }
        }
    }
    println!(
        "turned: #{k} decided alone, {hit} of {} cells",
        on(&tops[k])
    );
    assert!(hit * 10 > on(&tops[k]) * 8, "turned: the same top");

    // The woman of the portrait pasted onto the group: asked
    // about, empty, the people offered to pick from.
    let g = develop("5M0A4169", &plain, 0, &mut ai);
    let made = ai
        .raster(
            g.stamp,
            &g.image,
            &g.edit,
            g.source,
            g.turn,
            (3, 0),
            &part("Lips", Some(her.clone())),
            None,
        )
        .unwrap();
    match &made.part {
        Some(Unresolved::Ask(ask)) => {
            println!("pasted onto the group: asked, guess {:?}", ask.guess);
            assert_eq!(ask.people.len(), 5);
            assert_eq!(ask.why, crate::ai::Asking::Unsure);
        }
        other => panic!("pasted onto the group: {other:?}"),
    }
    assert!(made.raster.data().iter().all(|&v| v == 0));
    // And onto her other frame: found again, her lips.
    let o = develop("DSCF0848", &plain, 0, &mut ai);
    let made = ai
        .raster(
            o.stamp,
            &o.image,
            &o.edit,
            o.source,
            o.turn,
            (4, 0),
            &part("Lips", Some(her)),
            None,
        )
        .unwrap();
    println!(
        "pasted onto DSCF0848: {}, {:.3}% of the frame in {:.2}s",
        if made.part.is_none() {
            "decided"
        } else {
            "not decided"
        },
        100.0 * share(&made.raster),
        made.seconds
    );
    save("DSCF0848-lips-pasted", &made.raster);
    assert!(made.part.is_none() && share(&made.raster) > 0.0);
    // A group frame's person on the portrait: someone else, nobody.
    let b = develop("DSCF0835", &plain, 0, &mut ai);
    let made = ai
        .raster(
            b.stamp,
            &b.image,
            &b.edit,
            b.source,
            b.turn,
            (5, 0),
            &part("Hair", Some(group[0].person())),
            None,
        )
        .unwrap();
    println!(
        "group #0 pasted onto DSCF0835: {}",
        match &made.part {
            None => "decided".to_string(),
            Some(Unresolved::Nobody) => "nobody".to_string(),
            Some(Unresolved::Ask(a)) => format!("asked ({:?}), guess {:?}", a.why, a.guess),
        }
    );
    // Not her: asked as someone else's, the portrait's one face
    // outlined and no guess, never the stranger decided alone.
    match &made.part {
        Some(Unresolved::Ask(ask)) => {
            assert_eq!(ask.why, crate::ai::Asking::SomeoneElse);
            assert_eq!((ask.people.len(), ask.guess), (1, None));
        }
        other => panic!("group #0 pasted onto DSCF0835: {other:?}"),
    }
    assert!(made.raster.data().iter().all(|&v| v == 0));

    // A couple's person pasted onto the other couple: asked.
    let mut couples = Vec::new();
    for stem in ["4Z4A3846", "5M0A0504"] {
        let c = develop(stem, &plain, 0, &mut ai);
        let (people, _, _) = ai.people(c.stamp, &c.image, c.source, c.turn).unwrap();
        println!("{stem}: {} people", people.len());
        dump(stem, &people);
        couples.push((c, people));
    }
    let (c, _) = &couples[1];
    let made = ai
        .raster(
            c.stamp,
            &c.image,
            &c.edit,
            c.source,
            c.turn,
            (6, 0),
            &part("Lips", Some(couples[0].1[0].person())),
            None,
        )
        .unwrap();
    match &made.part {
        Some(Unresolved::Ask(ask)) => {
            println!("4Z4A3846 #0 onto 5M0A0504: asked, guess {:?}", ask.guess);
            assert_eq!(ask.why, crate::ai::Asking::Unsure);
        }
        other => panic!("4Z4A3846 #0 onto 5M0A0504: {other:?}"),
    }
}

/// The viewport against the export with the Dehaze on, on real
/// frames (`GREYCARD_SAMPLES`, a folder of raws; `GREYCARD_DEHAZE_ONLY`,
/// stems to keep, comma-separated). For each frame and each Detail
/// setting, the export's picture is developed on the CPU and the
/// viewport's on the GPU from the same base, so the CA correction
/// is the CPU's on both and what differs is the Detail section and
/// the sharpen alone. Both are finished the export's way (the edit's
/// look, sRGB, eight bits) and compared level by level; the dehaze's
/// airlight and mean transmission from each path are printed beside
/// them. A measurement, not a pass: it prints and holds nothing.
#[test]
#[ignore]
fn the_dehaze_viewport_against_the_export_on_real_frames() {
    let Some(samples) = std::env::var_os("GREYCARD_SAMPLES") else {
        eprintln!("set GREYCARD_SAMPLES to a folder of raws");
        return;
    };
    let only: Option<Vec<String>> = std::env::var("GREYCARD_DEHAZE_ONLY")
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect());
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the dehaze's viewport against its export");
        return;
    };
    let mut gpu = Some(ctx);
    let mut frames: Vec<PathBuf> = std::fs::read_dir(&samples)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| greycard_core::decode::is_raw_path(p))
        .collect();
    frames.sort();
    let deliver: Deliver = Arc::new(|_| {});
    let detail = |dehaze: f32, texture: f32, clarity: f32, sharpen: bool| {
        let mut edit = Edit::default();
        edit.detail.enabled = true;
        edit.detail.dehaze = dehaze;
        edit.detail.texture = texture;
        edit.detail.clarity = clarity;
        edit.sharpen.enabled = sharpen;
        edit
    };
    let cases = [
        ("dehaze +50", detail(0.5, 0.0, 0.0, true)),
        ("dehaze +60", detail(0.6, 0.0, 0.0, true)),
        (
            "dehaze +50, texture and clarity +50",
            detail(0.5, 0.5, 0.5, true),
        ),
        ("dehaze -50, clarity +30", detail(-0.5, 0.0, 0.3, true)),
        ("dehaze +100, sharpen off", detail(1.0, 0.0, 0.0, false)),
    ];
    for path in frames {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        if only.as_ref().is_some_and(|o| !o.contains(&stem)) {
            continue;
        }
        let Ok((input, _)) = open(&path) else {
            println!("{stem}: passed over, it will not open");
            continue;
        };
        let mut base = None;
        for (name, edit) in &cases {
            let run = |base: &mut Option<Base>, gpu: &mut Option<greycard_gpu::Context>| {
                develop_job(
                    &input,
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
            let (cpu_outcome, cpu) = run(&mut base, &mut None);
            let (gpu_outcome, _) = run(&mut base, &mut gpu);
            assert!(gpu.is_some(), "{stem}: the GPU failed");
            let cpu = cpu.expect("the CPU's develop gives its picture");
            let Outcome::Developed {
                dehaze: cpu_dehaze, ..
            } = cpu_outcome
            else {
                panic!("{stem}: the CPU's develop failed");
            };
            let Outcome::Developed {
                image,
                dehaze: gpu_dehaze,
                ..
            } = gpu_outcome
            else {
                panic!("{stem}: the GPU's develop failed");
            };
            let viewport = match image {
                Developed::Texture(t) => read_viewport(gpu.as_ref().unwrap(), &t),
                Developed::Halves(h) => {
                    let data: Vec<f32> = h
                        .pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|px| [px[0].to_f32(), px[1].to_f32(), px[2].to_f32()])
                        .collect();
                    WorkingImage::from_data(h.width as usize, h.height as usize, data).unwrap()
                }
            };
            let b = base.as_ref().unwrap();
            let global = crate::finish::Baked::global(edit, b.source);
            let to_out = crate::export::Space::Srgb.matrix();
            let finish = |image: &WorkingImage| -> Vec<u8> {
                crate::finish::finish_with(
                    image,
                    None,
                    &global,
                    &[],
                    |x, y| (x as f32, y as f32),
                    |_, _| (0.0, None),
                    None,
                    None,
                    None,
                    &to_out,
                    |v| (v.clamp(0.0, 1.0) * 255.0).round() as u8,
                )
            };
            let (a, v) = (finish(&cpu), finish(&viewport));
            // The sharpen's dots in the blacks (§268): a pixel whose
            // brightest channel is over 0.8 in a 16-bit finish while
            // the 5x5 mean of its Rec.709 luma is under 0.08.
            let dots = |image: &WorkingImage| -> usize {
                let out: Vec<u16> = crate::finish::finish_with(
                    image,
                    None,
                    &global,
                    &[],
                    |x, y| (x as f32, y as f32),
                    |_, _| (0.0, None),
                    None,
                    None,
                    None,
                    &to_out,
                    |v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16,
                );
                let (w, h) = (image.width, image.height);
                let at = |i: usize| f32::from(out[i]) / 65535.0;
                let luma: Vec<f32> = (0..w * h)
                    .map(|i| 0.2126 * at(i * 3) + 0.7152 * at(i * 3 + 1) + 0.0722 * at(i * 3 + 2))
                    .collect();
                (0..w * h)
                    .filter(|&i| {
                        let (x, y) = (i % w, i / w);
                        let brightest = at(i * 3).max(at(i * 3 + 1)).max(at(i * 3 + 2));
                        if brightest <= 0.8 {
                            return false;
                        }
                        let (mut sum, mut n) = (0f32, 0f32);
                        for yy in y.saturating_sub(2)..(y + 3).min(h) {
                            for xx in x.saturating_sub(2)..(x + 3).min(w) {
                                sum += luma[yy * w + xx];
                                n += 1.0;
                            }
                        }
                        sum / n < 0.08
                    })
                    .count()
            };
            let mut counts = [0usize; 256];
            let mut sq = 0f64;
            for (x, y) in a.iter().zip(&v) {
                let d = x.abs_diff(*y);
                counts[d as usize] += 1;
                sq += f64::from(d) * f64::from(d);
            }
            let n = a.len();
            let worst = counts.iter().rposition(|&c| c > 0).unwrap_or(0);
            let off = n - counts[0];
            let mut seen = 0;
            let p999 = counts
                .iter()
                .position(|&c| {
                    seen += c;
                    seen as f64 >= 0.999 * n as f64
                })
                .unwrap_or(0);
            let fit = |d: &Option<DehazeStats>| {
                d.map_or("none".into(), |d| {
                    format!(
                        "A ({:.5}, {:.5}, {:.5}) t {:.5}",
                        d.airlight[0], d.airlight[1], d.airlight[2], d.transmission_mean
                    )
                })
            };
            println!(
                "{stem} {}x{}, {name}: {:.4}% of samples off, {:.4}% by 2 or more, \
                 99.9th percentile {p999}, worst {worst} levels, RMSE {:.4} levels; \
                 export {} viewport {}; dots, export {} viewport {}",
                cpu.width,
                cpu.height,
                100.0 * off as f64 / n as f64,
                100.0 * (off - counts[1]) as f64 / n as f64,
                (sq / n as f64).sqrt(),
                fit(&cpu_dehaze),
                fit(&gpu_dehaze),
                dots(&cpu),
                dots(&viewport),
            );
        }
    }
}

/// The viewport's half-float texture read back as a working image,
/// its alpha dropped.
fn read_viewport(
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
