//! A pan right after a Detail move, timed on the device: an ignored
//! measurement of what the window's frames wait for while the settle
//! runs on the queue they share.
//!
//! A thread stands in for the window: every 60th of a second it copies
//! the view's rectangle of the picture on screen into a target of its
//! own on the worker's queue and waits for that copy alone, as a
//! present waits behind whatever the queue holds ahead of it. Once the
//! move's first picture lands it pans at 1:1, telling the worker what
//! it shows each frame as the window does.
//!
//! `GREYCARD_PAN_SIZE` (`WxH`, default 6000x4000), `GREYCARD_PAN_SECS`
//! (default 8), `GREYCARD_PAN_SEEN=0` (tell the worker nothing: the
//! whole develop first, as before the first round), and
//! `GREYCARD_PAN_OUT` (a folder for `frames.csv`, `events.csv`, and
//! `summary.txt`).

use super::engine::SETTLE_COUNTS;
use super::*;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// The view, physical pixels.
const VIEW: (u32, u32) = (2560, 1600);

/// The pan's speed, picture pixels a second at 1:1.
const SPEED: f32 = 1200.0;

const FRAME: Duration = Duration::from_micros(16_667);

/// A develop delivered: when (ms), its generation, whether a settle
/// follows, and its sharpen's iterations.
type Event = (f64, u64, bool, usize);

/// A frame of the window: when it was sent (ms), how long it waited
/// (ms), the time since the last (ms), and the phase.
type Shown = (f64, f64, f64, Phase);

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Waiting,
    Panning,
    After,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Waiting => "waiting",
            Phase::Panning => "panning",
            Phase::After => "after",
        }
    }
}

fn env_or<T: std::str::FromStr>(name: &str, or: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(or)
}

fn stats(of: &[f64]) -> String {
    if of.is_empty() {
        return "none".into();
    }
    let mut v = of.to_vec();
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    format!(
        "n {} p50 {:.1} p95 {:.1} max {:.1} ms, over 50 ms {}, over 100 ms {}",
        v.len(),
        at(0.5),
        at(0.95),
        v[v.len() - 1],
        v.iter().filter(|&&x| x > 50.0).count(),
        v.iter().filter(|&&x| x > 100.0).count()
    )
}

#[test]
#[ignore = "a measurement: run it on purpose, GREYCARD_PAN_OUT for its log"]
fn a_pan_after_a_detail_move() {
    let (w, h) = std::env::var("GREYCARD_PAN_SIZE")
        .ok()
        .and_then(|s| {
            let (a, b) = s.split_once('x')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((6000usize, 4000usize));
    let secs: f32 = env_or("GREYCARD_PAN_SECS", 8.0);
    let tell = std::env::var("GREYCARD_PAN_SEEN").map_or(true, |v| v != "0");
    // The move made at a fit view, the view zoomed to 1:1 as its first
    // round lands, and the pan from there.
    // `GREYCARD_PAN_ZOOM=wheel`: from the fit view through a wheel's
    // steps, 0.26, 0.4, and 0.6, a sixth of a second each, to 1:1.
    let wheel = std::env::var("GREYCARD_PAN_ZOOM").is_ok_and(|v| v == "wheel");
    let zoom = wheel || std::env::var("GREYCARD_PAN_ZOOM").is_ok_and(|v| v == "1");
    let Ok(ctx) = greycard_gpu::Context::own() else {
        super::tests::skipped("the pan's measurement");
        return;
    };
    let dir = crate::testing::scratch_dir("pan");
    let path = dir.join("a.dng");
    crate::testing::linear_dng(&path, w, h, "Test", "Pan");
    let t0 = Instant::now();
    let ms = move |t: Instant| t.duration_since(t0).as_secs_f64() * 1e3;
    let latest: Arc<Mutex<Option<greycard_gpu::wgpu::Texture>>> = Arc::default();
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let (tx, heard) = std::sync::mpsc::channel::<(u64, bool)>();
    let worker = {
        let (latest, events, tx) = (latest.clone(), events.clone(), Mutex::new(tx));
        Worker::new(move |o| {
            if let Outcome::Developed {
                generation,
                sharpen,
                settles,
                image,
                ..
            } = o
            {
                events.lock().unwrap().push((
                    ms(Instant::now()),
                    generation,
                    settles,
                    sharpen.map_or(0, |s| s.iterations),
                ));
                if let Developed::Texture(t) = image {
                    *latest.lock().unwrap() = Some(t);
                }
                let _ = tx.lock().unwrap().send((generation, settles));
            }
        })
    };
    // The working sets kept between runs or made for each, against the
    // device's default (`GREYCARD_PAN_KEEP`, 1 or 0).
    let own = greycard_gpu::Context::from_device(ctx.device(), ctx.queue()).expect("a context");
    if let Ok(keep) = std::env::var("GREYCARD_PAN_KEEP") {
        own.keep_sharpen_planes(keep == "1");
        own.keep_local_contrast_planes(keep == "1");
    }
    let keeps = own.keeps_sharpen_planes();
    worker.set_context(own);
    worker.set_thumb_cache(Some(
        greycard_library::thumbs::Thumbs::at(
            dir.join("thumbs"),
            greycard_library::thumbs::DEFAULT_CAP,
        )
        .with_previews(
            crate::edited_pictures::BIG,
            greycard_library::thumbs::DEFAULT_PREVIEW_CAP,
        ),
    ));
    let long = Duration::from_secs(1200);
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
    assert_eq!(heard.recv_timeout(long).expect("opened").0, 1);
    let opened = ms(Instant::now());
    let (vw, vh) = VIEW;
    let seen_at = move |x: f32, y: f32| Seen {
        rect: [
            x / w as f32,
            y / h as f32,
            (x + vw as f32) / w as f32,
            (y + vh as f32) / h as f32,
        ],
        zoom: 1.0,
    };
    let start = (
        (w as f32 - vw as f32).max(0.0) / 2.0,
        (h as f32 - vh as f32).max(0.0) / 2.0,
    );
    // The view about the picture's middle at `z`.
    let centered = move |z: f32| {
        let (hw, hh) = (
            vw as f32 / z / 2.0 / w as f32,
            vh as f32 / z / 2.0 / h as f32,
        );
        Seen {
            rect: [
                (0.5 - hw).max(0.0),
                (0.5 - hh).max(0.0),
                (0.5 + hw).min(1.0),
                (0.5 + hh).min(1.0),
            ],
            zoom: z,
        }
    };
    const WHEEL: [f32; 3] = [0.26, 0.4, 0.6];
    const STEP: u32 = 10;
    let fit = Seen {
        rect: [0.0, 0.0, 1.0, 1.0],
        zoom: (vw as f32 / w as f32).min(vh as f32 / h as f32),
    };
    if tell && zoom {
        worker.set_seen(Some(fit));
    } else if tell {
        worker.set_seen(Some(seen_at(start.0, start.1)));
    }
    let before = SETTLE_COUNTS.each();

    // The window.
    let phase = Arc::new(Mutex::new(Phase::Idle));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let frames: Arc<Mutex<Vec<Shown>>> = Arc::default();
    let window = {
        let (device, queue) = (ctx.device().clone(), ctx.queue().clone());
        let (latest, phase, stop, frames) =
            (latest.clone(), phase.clone(), stop.clone(), frames.clone());
        let seen_to = tell.then(|| worker.seen_setter());
        std::thread::spawn(move || {
            let target = device.create_texture(&greycard_gpu::wgpu::TextureDescriptor {
                label: Some("the window"),
                size: greycard_gpu::wgpu::Extent3d {
                    width: vw,
                    height: vh,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: greycard_gpu::wgpu::TextureDimension::D2,
                format: greycard_gpu::wgpu::TextureFormat::Rgba16Float,
                usage: greycard_gpu::wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let (mut x, y) = start;
            let mut dir = 1.0f32;
            let mut next = Instant::now();
            let mut last_done: Option<Instant> = None;
            let mut stepped = 0u32;
            let max_x = (w as f32 - vw as f32).max(0.0);
            while !stop.load(Ordering::Relaxed) {
                let now_phase = *phase.lock().unwrap();
                if matches!(now_phase, Phase::Panning) && wheel && stepped < STEP * 3 {
                    if let Some(set) = &seen_to {
                        set(Some(centered(WHEEL[(stepped / STEP) as usize])));
                    }
                    stepped += 1;
                } else if matches!(now_phase, Phase::Panning) {
                    x += dir * SPEED * FRAME.as_secs_f32();
                    if x >= max_x || x <= 0.0 {
                        dir = -dir;
                        x = x.clamp(0.0, max_x);
                    }
                    if let Some(set) = &seen_to {
                        set(Some(seen_at(x, y)));
                    }
                }
                let mut encoder = device.create_command_encoder(&Default::default());
                if let Some(t) = latest.lock().unwrap().clone() {
                    let (cw, ch) = (vw.min(t.width()), vh.min(t.height()));
                    let ox = (x as u32).min(t.width() - cw);
                    let oy = (y as u32).min(t.height() - ch);
                    encoder.copy_texture_to_texture(
                        greycard_gpu::wgpu::TexelCopyTextureInfo {
                            texture: &t,
                            mip_level: 0,
                            origin: greycard_gpu::wgpu::Origin3d { x: ox, y: oy, z: 0 },
                            aspect: greycard_gpu::wgpu::TextureAspect::All,
                        },
                        target.as_image_copy(),
                        greycard_gpu::wgpu::Extent3d {
                            width: cw,
                            height: ch,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                let submitted = Instant::now();
                let index = queue.submit(Some(encoder.finish()));
                let _ = device.poll(greycard_gpu::wgpu::PollType::Wait {
                    submission_index: Some(index),
                    timeout: None,
                });
                let done = Instant::now();
                let interval =
                    last_done.map_or(0.0, |l| done.duration_since(l).as_secs_f64() * 1e3);
                last_done = Some(done);
                frames.lock().unwrap().push((
                    ms(submitted),
                    done.duration_since(submitted).as_secs_f64() * 1e3,
                    interval,
                    now_phase,
                ));
                next += FRAME;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
        })
    };
    std::thread::sleep(Duration::from_secs(1));

    // The move, and the pan once its first picture lands.
    *phase.lock().unwrap() = Phase::Waiting;
    let moved = Instant::now();
    let mut edit = Edit::default();
    edit.detail.clarity = 0.4;
    worker.send(Job::Develop {
        edit,
        generation: 2,
        turn: 0,
        of: None,
    });
    let mut first = None;
    let mut settled = None;
    let mut first_round = 0;
    while first.is_none() {
        let (g, settles) = heard.recv_timeout(long).expect("the move lands");
        if g == 2 {
            first = Some(moved.elapsed());
            first_round += usize::from(settles);
            if !settles {
                settled = first;
            }
        }
    }
    if tell && zoom && !wheel {
        worker.set_seen(Some(seen_at(start.0, start.1)));
    }
    let mut zoomed_round = None;
    *phase.lock().unwrap() = Phase::Panning;
    let panning = Instant::now();
    let pan_for = Duration::from_secs_f32(secs);
    while settled.is_none() || panning.elapsed() < pan_for {
        if panning.elapsed() >= pan_for {
            *phase.lock().unwrap() = Phase::After;
        }
        match heard.recv_timeout(Duration::from_millis(20)) {
            Ok((2, settles)) => {
                first_round += usize::from(settles);
                if settles && zoomed_round.is_none() {
                    zoomed_round = Some(panning.elapsed());
                }
                if !settles {
                    settled = Some(moved.elapsed());
                }
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                assert!(moved.elapsed() < long, "the settle lands");
            }
            Err(e) => panic!("{e}"),
        }
    }
    *phase.lock().unwrap() = Phase::After;
    std::thread::sleep(Duration::from_secs(1));
    stop.store(true, Ordering::Relaxed);
    window.join().unwrap();
    let after = SETTLE_COUNTS.each();
    worker.stop_all(Duration::from_secs(600));

    let frames = frames.lock().unwrap().clone();
    let events = events.lock().unwrap().clone();
    let of = |p: &str, pick: fn(&Shown) -> f64| {
        frames
            .iter()
            .filter(|f| f.3.name() == p)
            .map(pick)
            .collect::<Vec<_>>()
    };
    let panning_at = ms(panning);
    let pan_end = panning_at + f64::from(secs) * 1e3;
    let in_pan = |pick: fn(&Shown) -> f64| {
        frames
            .iter()
            .filter(|f| f.0 >= panning_at && f.0 < pan_end)
            .map(pick)
            .collect::<Vec<_>>()
    };
    let settled_at = settled.map(|s| ms(moved) + s.as_secs_f64() * 1e3);
    let after_settle = |pick: fn(&Shown) -> f64| {
        frames
            .iter()
            .filter(|f| settled_at.is_some_and(|s| f.0 > s + 100.0))
            .map(pick)
            .collect::<Vec<_>>()
    };
    let shown_at = ms(moved) + first.unwrap().as_secs_f64() * 1e3;
    let settling = |pick: fn(&Shown) -> f64| {
        frames
            .iter()
            .filter(|f| f.0 > shown_at && settled_at.is_some_and(|s| f.0 < s))
            .map(pick)
            .collect::<Vec<_>>()
    };
    let summary = format!(
        "frame {w}x{h}, adapter {}, seen told {tell}, sets kept {keeps}, \
         moved at fit and zoomed to 1:1 {zoom}, by the wheel {wheel}, pan {secs} s at {SPEED} px/s\n\
         open landed at {opened:.0} ms\n\
         first show after the move {:.0} ms\n\
         settled after the move {:.0} ms\n\
         first rounds delivered {first_round}\n\
         settles begun {}, given way {}, first rounds for a zoom in {}\n\
         a first round after the pan began {:?} ms\n\
         idle frames latency: {}\n\
         waiting frames latency: {}\n\
         pan ({secs} s) frames latency: {}\n\
         pan ({secs} s) frame interval: {}\n\
         pan frames shown {} of {:.0} asked\n\
         after the settle, latency: {}\n\
         from the first show to the settle, frame interval: {}\n",
        ctx.device().adapter_info().name,
        first.unwrap().as_secs_f64() * 1e3,
        settled.unwrap().as_secs_f64() * 1e3,
        after[0] - before[0],
        after[1] - before[1],
        after[2] - before[2],
        zoomed_round.map(|t| (t.as_secs_f64() * 1e3).round()),
        stats(&of("idle", |f| f.1)),
        stats(&of("waiting", |f| f.1)),
        stats(&in_pan(|f| f.1)),
        stats(&in_pan(|f| f.2)),
        in_pan(|f| f.1).len(),
        f64::from(secs) * 60.0,
        stats(&after_settle(|f| f.1)),
        stats(&settling(|f| f.2)),
    );
    println!("{summary}");
    if let Some(out) = std::env::var_os("GREYCARD_PAN_OUT") {
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("summary.txt"), &summary).unwrap();
        let mut csv = String::from("t_ms,latency_ms,interval_ms,phase\n");
        for f in &frames {
            csv += &format!("{:.1},{:.2},{:.2},{}\n", f.0, f.1, f.2, f.3.name());
        }
        std::fs::write(out.join("frames.csv"), csv).unwrap();
        let mut csv = String::from("t_ms,generation,settles,iterations\n");
        for e in &events {
            csv += &format!("{:.1},{},{},{}\n", e.0, e.1, e.2, e.3);
        }
        std::fs::write(out.join("events.csv"), csv).unwrap();
    }
    crate::testing::remove_dir_retry(&dir);
}
