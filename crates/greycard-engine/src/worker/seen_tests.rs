//! The window's develop sharpened first as far as the window shows it,
//! and the settle after it, on the device (lavapipe in CI).

use super::develop::{Round, develop_round};
use super::seen::{Seen, part_of};
use super::tests::{aberrated_frame, skipped};
use super::*;
use crate::walk::{Hold, NoHold};
use greycard_core::develop::sharpen::{self, Part};

const W: usize = 640;
const H: usize = 480;

/// One round of `edit` on the frame kept `base`, delivering nothing.
fn go(
    input: &Input,
    edit: &Edit,
    base: &mut Option<FrameKept>,
    gpu: &mut Option<greycard_gpu::Context>,
    round: &mut Round,
    hold: &dyn Hold,
) -> Outcome {
    let deliver: Deliver = Arc::new(|_| {});
    develop_round(
        input,
        edit,
        0,
        1,
        base,
        &mut Ai::new(),
        (None, None),
        gpu,
        &deliver,
        round,
        hold,
    )
    .0
}

/// The texture an outcome carries, read back as half floats, eight
/// bytes a pixel; and its sharpen's stats.
fn shown(gpu: &Option<greycard_gpu::Context>, outcome: &Outcome) -> (Vec<u8>, SharpenStats) {
    let Outcome::Developed {
        image: Developed::Texture(texture),
        sharpen: Some(stats),
        ..
    } = outcome
    else {
        panic!("a texture, sharpened");
    };
    let ctx = gpu.as_ref().expect("a device");
    let rows = ctx
        .read_viewport_rows(texture, 0, texture.height())
        .expect("reads back");
    (rows, *stats)
}

/// Whether the pixel at `x`, `y` is in a tile `part` touches.
fn touched(part: &Part, x: usize, y: usize) -> bool {
    let (cols, bands) = part.tiles(W, H, sharpen::tile_for(H));
    let tile = sharpen::tile_for(H);
    cols.contains(&(x / tile)) && bands.contains(&(y / tile))
}

/// Clarity at `amount`, the sharpen's point spread the editor's
/// default: the synthetic frame measures one far wider, which a fit
/// view would need every iteration of.
fn clarity(amount: f32) -> Edit {
    let mut edit = Edit::default();
    edit.detail.clarity = amount;
    edit.sharpen.auto_radius = false;
    edit
}

const CLOSE: Seen = Seen {
    rect: [0.3, 0.3, 0.6, 0.6],
    zoom: 1.0,
};

const FIT: Seen = Seen {
    rect: [0.0, 0.0, 1.0, 1.0],
    zoom: 0.25,
};

/// A Clarity move seen at 1:1 on part of the frame: the first round's
/// tiles on screen are the whole develop's to the bit and the rest is
/// left unsharpened; the settle after it is the whole develop's
/// everywhere, from the point kept before the sharpen. Seen at a fit
/// view, the first round sharpens every tile at fewer iterations, and
/// its settle is again the whole develop's.
#[test]
fn the_first_round_sharpens_what_is_seen_and_the_settle_the_rest() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the view's sharpen");
        return;
    };
    let input = Input::Raw(Arc::new(aberrated_frame(W, H)));
    let mut gpu = Some(ctx);
    // The whole develops, each from nothing.
    let whole = |edit: &Edit, gpu: &mut Option<greycard_gpu::Context>| {
        let outcome = go(&input, edit, &mut None, gpu, &mut Round::first(), &NoHold);
        shown(gpu, &outcome)
    };
    let (full, full_stats) = whole(&clarity(0.4), &mut gpu);
    let (other, other_stats) = whole(&clarity(-0.3), &mut gpu);
    assert_eq!(full_stats.iterations, 20);
    // The frame as the window has it, then the move seen close up.
    let mut base = None;
    let mut round = Round::first();
    go(
        &input,
        &Edit::default(),
        &mut base,
        &mut gpu,
        &mut round,
        &NoHold,
    );
    assert!(round.quick.is_none(), "an open's develop is whole");
    let mut round = Round::first_seen(Some(CLOSE));
    let quick = go(
        &input,
        &clarity(0.4),
        &mut base,
        &mut gpu,
        &mut round,
        &NoHold,
    );
    assert!(round.quick.is_some());
    let (rows, stats) = shown(&gpu, &quick);
    assert_eq!(stats, full_stats, "the stats are the whole picture's");
    let part = part_of(&CLOSE, (W, H)).expect("a part");
    let (mut inside, mut apart) = (0, 0);
    for y in 0..H {
        for x in 0..W {
            let at = (y * W + x) * 8..(y * W + x + 1) * 8;
            if touched(&part, x, y) {
                assert_eq!(rows[at.clone()], full[at], "{x},{y}");
                inside += 1;
            } else {
                apart += usize::from(rows[at.clone()] != full[at]);
            }
        }
    }
    assert!(inside > 0 && inside < W * H);
    assert!(
        apart > 0,
        "outside the part, the picture before the sharpen"
    );
    let b = base.as_ref().unwrap();
    assert!(b.kept.on_device(crate::stages::Keep::PreSharpen));
    let mut round = Round::settle();
    let settled = go(
        &input,
        &clarity(0.4),
        &mut base,
        &mut gpu,
        &mut round,
        &NoHold,
    );
    assert!(round.quick.is_none() && !round.gave_way);
    let (rows, stats) = shown(&gpu, &settled);
    assert_eq!(stats, full_stats);
    assert!(rows == full, "the settle is the whole develop's");
    // Seen at a fit view: every tile, fewer iterations.
    let mut round = Round::first_seen(Some(FIT));
    let quick = go(
        &input,
        &clarity(-0.3),
        &mut base,
        &mut gpu,
        &mut round,
        &NoHold,
    );
    assert!(round.quick.is_some());
    let (rows, stats) = shown(&gpu, &quick);
    assert_eq!(
        stats.iterations,
        sharpen::iterations_seen_at(FIT.zoom, stats.radius, 20)
    );
    assert!(stats.iterations < 20);
    assert!(rows != other);
    let mut round = Round::settle();
    let settled = go(
        &input,
        &clarity(-0.3),
        &mut base,
        &mut gpu,
        &mut round,
        &NoHold,
    );
    let (rows, stats) = shown(&gpu, &settled);
    assert_eq!(stats, other_stats);
    assert!(rows == other, "the settle is the whole develop's");
    // And no view: the whole develop in the first round, nothing to
    // settle.
    let mut round = Round::first_seen(None);
    let plain = go(
        &input,
        &clarity(0.4),
        &mut base,
        &mut gpu,
        &mut round,
        &NoHold,
    );
    assert!(round.quick.is_none());
    assert!(shown(&gpu, &plain).0 == full);
}

/// A hold that gives way before the sharpen, or inside it after its
/// first submission, counting what it is asked.
struct Asks {
    before: bool,
    inside: std::cell::Cell<u32>,
}

impl Hold for Asks {
    fn give_way(&self, next: &crate::stages::Stage) -> bool {
        self.before && matches!(next, crate::stages::Stage::Sharpen(..))
    }

    fn give_way_inside(&self, _stage: &crate::stages::Stage) -> bool {
        self.inside.set(self.inside.get() + 1);
        true
    }
}

/// The settle gives way to a newer develop before its sharpen and
/// inside it, between its submissions, delivering nothing and leaving
/// the point before the sharpen kept; the settle after that is the
/// whole develop's.
#[test]
fn the_settle_gives_way_before_and_inside_the_sharpen() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the settle's give-way");
        return;
    };
    // A submission an iteration a band of tiles: room to stop.
    ctx.set_sharpen_batching(greycard_gpu::Batching::Every(1));
    ctx.set_sharpen_atlas_budget(4);
    let input = Input::Raw(Arc::new(aberrated_frame(W, H)));
    let mut gpu = Some(ctx);
    let edit = clarity(0.4);
    let outcome = go(
        &input,
        &edit,
        &mut None,
        &mut gpu,
        &mut Round::first(),
        &NoHold,
    );
    let (full, _) = shown(&gpu, &outcome);
    let mut base = None;
    let mut round = Round::first_seen(Some(CLOSE));
    go(&input, &edit, &mut base, &mut gpu, &mut round, &NoHold);
    assert!(round.quick.is_some());
    for before in [true, false] {
        let asks = Asks {
            before,
            inside: std::cell::Cell::new(0),
        };
        let mut round = Round::settle();
        let outcome = go(&input, &edit, &mut base, &mut gpu, &mut round, &asks);
        assert!(matches!(outcome, Outcome::Failed { .. }) && round.gave_way);
        assert_eq!(asks.inside.get(), u32::from(!before), "before {before}");
        let b = base.as_ref().unwrap();
        assert!(b.kept.on_device(crate::stages::Keep::PreSharpen));
    }
    let mut round = Round::settle();
    let settled = go(&input, &edit, &mut base, &mut gpu, &mut round, &NoHold);
    assert!(shown(&gpu, &settled).0 == full);
}

/// The worker on its thread, the window showing a fit view: a develop
/// lands sharpened at the fewer iterations, then settles at the whole;
/// a develop asked while the settle runs takes its place and settles in
/// turn, and nothing more is said. An open of another frame asked then
/// leaves the old frame's settle unmade.
#[test]
fn a_newer_develop_or_open_takes_the_settles_place() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the worker's settle");
        return;
    };
    let dir = crate::testing::scratch_dir("seen-settle");
    let (a, b) = (dir.join("a.dng"), dir.join("b.dng"));
    crate::testing::linear_dng(&a, W, H, "Test", "Cam");
    crate::testing::linear_dng(&b, W, H, "Test", "Cam");
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed {
            generation,
            sharpen,
            ..
        } = o
        {
            let iterations = sharpen.map_or(0, |s| s.iterations);
            tx.lock().unwrap().send((generation, iterations)).unwrap();
        }
    });
    worker.set_context(ctx);
    let wait = std::time::Duration::from_secs(120);
    let next = || rx.recv_timeout(wait).expect("a delivery");
    let open = |path: &std::path::Path, generation| Job::Open {
        path: path.to_path_buf(),
        version: None,
        edit: Edit::default(),
        generation,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    };
    let develop = |amount, generation| Job::Develop {
        edit: clarity(amount),
        generation,
        turn: 0,
        of: None,
    };
    worker.send(open(&a, 1));
    worker.set_seen(Some(FIT));
    assert_eq!(next(), (1, 20), "an open is the whole develop");
    // The settle's first ask finds a newer develop.
    worker.queue.0.lock().unwrap().develop_on_fill_ask = Some(develop(-0.3, 3));
    worker.send(develop(0.4, 2));
    let said: Vec<(u64, usize)> = (0..3).map(|_| next()).collect();
    let few = said[0].1;
    assert!(few < 20, "{said:?}");
    assert_eq!(said, [(2, few), (3, few), (3, 20)]);
    assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).is_err());
    // An open of the next frame while the settle is due.
    worker.queue.0.lock().unwrap().develop_on_fill_ask = Some(open(&b, 5));
    worker.send(develop(0.2, 4));
    let said: Vec<(u64, usize)> = (0..2).map(|_| next()).collect();
    assert_eq!(said, [(4, few), (5, 20)]);
    assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).is_err());
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// A save of the open frame while its develop's first round is on
/// screen waits for the settle: the edit's pictures kept are those of
/// a session that never sharpened a part first, to the byte, whatever
/// the window showed.
#[test]
fn a_save_keeps_the_settled_picture() {
    // Each run's file its own bytes, by its body's name: the edit's
    // pictures are known by the file's hash.
    let kept = |seen: Option<Seen>, body: &str| -> Option<Vec<u8>> {
        let ctx = greycard_gpu::Context::own().ok()?;
        let dir = crate::testing::scratch_dir("seen-save");
        let path = dir.join("a.dng");
        crate::testing::linear_dng(&path, W, H, "Test", body);
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let worker = Worker::new(move |o| {
            let said = match o {
                Outcome::Developed { generation, .. } => Some(generation),
                Outcome::EditedKept { .. } => None,
                _ => return,
            };
            let _ = tx.lock().unwrap().send(said);
        });
        worker.set_context(ctx);
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
        let wait = std::time::Duration::from_secs(120);
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
        worker.set_seen(seen);
        assert_eq!(rx.recv_timeout(wait).unwrap(), Some(1));
        let edit = clarity(0.4);
        // The save first: it waits for the develop of its edit, and
        // then, with the first round's picture on screen, for the settle.
        worker.send(Job::Keep {
            frame: Frame::file(path.clone()),
            edit: edit.clone(),
            turn: 0,
            iso: None,
        });
        worker.send(Job::Develop {
            edit: edit.clone(),
            generation: 2,
            turn: 0,
            of: None,
        });
        let mut said = Vec::new();
        loop {
            let s = rx
                .recv_timeout(wait)
                .unwrap_or_else(|_| panic!("the worker answers, {seen:?}, after {said:?}"));
            said.push(s);
            if s.is_none() {
                break;
            }
        }
        let developed = if seen.is_some() { 2 } else { 1 };
        assert_eq!(said.len(), developed + 1, "{said:?}");
        let edited = worker.edited();
        let hash = greycard_library::hash_file(&path).unwrap();
        let key = edited.key_of(&path, &edit);
        let big = edited
            .lookup(
                &hash,
                crate::edited_pictures::BIG,
                crate::edited_pictures::Shows::Edit(key),
            )
            .expect("the 2048 picture");
        worker.stop();
        crate::testing::remove_dir_retry(&dir);
        Some(big.rgb)
    };
    let Some(whole) = kept(None, "Cam") else {
        skipped("the save after the settle");
        return;
    };
    for (seen, body) in [(CLOSE, "Cam 2"), (FIT, "Cam 3")] {
        assert!(kept(Some(seen), body) == Some(whole.clone()), "{seen:?}");
    }
}

/// Wall time of a Clarity and a Dehaze move at 45 and 102 MP on the
/// device, each to its texture read back: the whole develop's, the
/// first round's seen at 1:1 in a 2560 by 1440 view and at the fit of
/// that view, and the settle after it. Sizes from
/// `GREYCARD_SEEN_SIZES` (`8256x5504,11648x8736` by default). A
/// measurement, printed: run in release.
#[test]
#[ignore = "a measurement, printed: run in release"]
fn a_detail_move_timed_at_45_and_102_megapixels() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the move's timing");
        return;
    };
    let sizes: Vec<(usize, usize)> = std::env::var("GREYCARD_SEEN_SIZES")
        .unwrap_or_else(|_| "8256x5504,11648x8736".into())
        .split(',')
        .filter_map(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .collect();
    // Runs a move, `GREYCARD_SEEN_RUNS` (3 by default), and the sliders
    // timed, `GREYCARD_SEEN_SLIDERS` (both by default).
    let n: usize = std::env::var("GREYCARD_SEEN_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let sliders =
        std::env::var("GREYCARD_SEEN_SLIDERS").unwrap_or_else(|_| "Clarity,Dehaze".into());
    let mut gpu = Some(ctx);
    for (w, h) in sizes {
        let input = Input::Raw(Arc::new(aberrated_frame(w, h)));
        let mut base = None;
        let mut edit = Edit::default();
        edit.sharpen.auto_radius = false;
        edit.detail.enabled = true;
        let started = Instant::now();
        go(
            &input,
            &edit,
            &mut base,
            &mut gpu,
            &mut Round::first(),
            &NoHold,
        );
        println!("{w}x{h} opened in {:.2} s", started.elapsed().as_secs_f64());
        let (vw, vh) = (2560.0, 1440.0);
        let fit = (vw / w as f32).min(vh / h as f32);
        let close = Seen {
            rect: [
                0.5 - vw / 2.0 / w as f32,
                0.5 - vh / 2.0 / h as f32,
                0.5 + vw / 2.0 / w as f32,
                0.5 + vh / 2.0 / h as f32,
            ],
            zoom: 1.0,
        };
        let far = Seen {
            rect: [0.0, 0.0, 1.0, 1.0],
            zoom: fit,
        };
        for slider in sliders.split(',') {
            let mut step = 0;
            let mut timed = |seen: Option<Seen>, settle: bool| {
                step += 1;
                let amount = if step % 2 == 0 { 0.3 } else { 0.5 };
                let mut edit = edit.clone();
                match slider {
                    "Clarity" => edit.detail.clarity = amount,
                    _ => edit.detail.dehaze = amount,
                }
                let started = Instant::now();
                let mut round = Round::first_seen(seen);
                let outcome = go(&input, &edit, &mut base, &mut gpu, &mut round, &NoHold);
                let _ = shown(&gpu, &outcome);
                let first = started.elapsed().as_secs_f64();
                let rest = (settle && round.quick.is_some()).then(|| {
                    let started = Instant::now();
                    let outcome = go(
                        &input,
                        &edit,
                        &mut base,
                        &mut gpu,
                        &mut Round::settle(),
                        &NoHold,
                    );
                    let _ = shown(&gpu, &outcome);
                    started.elapsed().as_secs_f64()
                });
                (first, rest)
            };
            // Warm: the planes made, the threshold read.
            timed(None, false);
            for (name, seen) in [("whole", None), ("1:1", Some(close)), ("fit", Some(far))] {
                let runs: Vec<(f64, Option<f64>)> = (0..n).map(|_| timed(seen, true)).collect();
                let median = |mut v: Vec<f64>| {
                    v.sort_by(f64::total_cmp);
                    v.get(v.len() / 2).copied()
                };
                let first = median(runs.iter().map(|r| r.0).collect()).unwrap_or(0.0);
                let rest = median(runs.iter().filter_map(|r| r.1).collect());
                println!(
                    "{w}x{h} {slider} {name}: to the frame {first:.2} s{}",
                    rest.map_or(String::new(), |s| format!(", the settle after it {s:.2} s"))
                );
            }
        }
    }
}
