//! The learned highlight fill delivered in rounds to the window (notes
//! §719 §3): the picture first without it, or with the last fields
//! standing in under another white balance, then the fill.

use std::cell::Cell;

use greycard_core::develop::highlight_fill::oklab;
use greycard_core::develop::highlight_fill::{Camera, FrameCodes};
use greycard_core::develop::{demosaic_prepared, finish, prepare_with};
use greycard_core::raw::{
    Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Orientation, RawFrame, Samples,
    SensorLayout,
};
use greycard_edit::WhiteBalance;

use super::*;
use crate::stages::{Keep, Purpose, Stage, stages};
use crate::walk::{Hold, NoHold};

/// A Bayer raw `w` by `h`: a ramp, a bright disk, and a fine checker,
/// its white at `white`: at 2000 the right of the ramp and the disk clip
/// on green and the disk's brightest on red too, and the fill lifts
/// them by thousandths of a stop; at 900 most of the frame's right half
/// clips, and the fill lifts the frame's top by about a stop.
fn clipped_raw(w: usize, h: usize, white: f32) -> RawFrame {
    let pattern = CfaPattern::rggb();
    let scene = |x: f32, y: f32| {
        let ramp = 200.0 + 2400.0 * x / w as f32;
        let (dx, dy) = (x - w as f32 * 0.6, y - h as f32 * 0.4);
        let r = (w.min(h) as f32 / 6.0).powi(2);
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
    RawFrame {
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
            white: LevelPattern::uniform(white, 1),
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
    }
}

fn input() -> (Arc<RawFrame>, Input) {
    input_at(2000.0)
}

fn input_at(white: f32) -> (Arc<RawFrame>, Input) {
    let raw = Arc::new(clipped_raw(320, 240, white));
    (raw.clone(), Input::Raw(raw))
}

/// An edit whose developed picture is its base: nothing after the base
/// moves a pixel.
fn plain() -> Edit {
    let mut edit = Edit::default();
    edit.sharpen.enabled = false;
    edit
}

fn on(mut edit: Edit) -> Edit {
    edit.highlight_fill = Some("hl-1".into());
    edit
}

fn at_kelvin(mut edit: Edit, temperature: f64) -> Edit {
    edit.white_balance = WhiteBalance::Custom {
        temperature,
        tint: 0.0,
    };
    edit
}

/// One round of the window's develop of `edit` on `base`.
struct Rounds {
    input: Input,
    ai: Ai,
    gpu: Option<greycard_gpu::Context>,
}

/// What a round delivered: its fills' report, and its picture in memory
/// (none on the device).
struct Landed {
    fills: FillReport,
    picture: Option<Arc<WorkingImage>>,
}

impl Rounds {
    fn new(input: &Input, gpu: Option<greycard_gpu::Context>) -> Self {
        Self {
            input: input.clone(),
            ai: Ai::for_test(greycard_ai::Store::at("no-store"), Vec::new()),
            gpu,
        }
    }

    fn go(
        &mut self,
        edit: &Edit,
        turn: u8,
        base: &mut Option<FrameKept>,
        round: &mut Round,
        hold: &dyn Hold,
    ) -> Option<Landed> {
        let deliver: Deliver = Arc::new(|_| {});
        let (outcome, picture) = develop_round(
            &self.input,
            edit,
            turn,
            1,
            base,
            &mut self.ai,
            (None, None),
            &mut self.gpu,
            &deliver,
            round,
            hold,
        );
        match outcome {
            Outcome::Developed { fills, .. } => Some(Landed { fills, picture }),
            Outcome::Failed { .. } if round.gave_way => None,
            _ => panic!("developed or gave way"),
        }
    }

    /// `edit` developed in one round from nothing kept: the export's
    /// way, the fill made in the walk.
    fn one_shot(&mut self, edit: &Edit) -> (Landed, FrameKept) {
        let mut base = None;
        let landed = self
            .go(edit, 0, &mut base, &mut Round::all(), &NoHold)
            .unwrap();
        (landed, base.unwrap())
    }
}

fn base_of(base: &Option<FrameKept>) -> Arc<WorkingImage> {
    base.as_ref()
        .and_then(|b| b.kept.picture(Keep::Base).cloned())
        .expect("a base kept")
}

fn same(a: &WorkingImage, b: &WorkingImage) -> bool {
    (a.width, a.height) == (b.width, b.height)
        && a.data
            .iter()
            .map(|v| v.to_bits())
            .eq(b.data.iter().map(|v| v.to_bits()))
}

/// The first round with the fill on delivers the edit's picture with the
/// fill off, to the bit, and keeps only that edit's points; the next
/// delivers the picture one walk with the fill makes, to the bit; a
/// round after keeps it and leaves nothing pending. On the CPU, and on
/// the device (lavapipe), where the pictures compared are the bases.
fn first_off_then_filled(gpu: Option<greycard_gpu::Context>) {
    let (_, input) = input();
    let on_gpu = gpu.is_some();
    let mut r = Rounds::new(&input, gpu);
    let (off, off_base) = r.one_shot(&plain());
    let (filled, filled_base) = r.one_shot(&on(plain()));
    let off_picture = off_base.kept.picture(Keep::Base).unwrap().clone();
    let filled_picture = filled_base.kept.picture(Keep::Base).unwrap().clone();
    assert!(
        !same(&off_picture, &filled_picture),
        "the fill moved pixels"
    );
    assert!(!off.fills.highlight_pending && !filled.fills.highlight_pending);

    let mut base = None;
    let mut round = Round::first();
    let first = r
        .go(&on(plain()), 0, &mut base, &mut round, &NoHold)
        .unwrap();
    assert!(first.fills.highlight_pending);
    let b = base.as_ref().unwrap();
    assert!(b.filling, "the edit's pictures wait for the fill");
    assert!(b.kept.fill().is_none(), "no fields made");
    assert!(!b.kept.made_by(|s| matches!(s, Stage::HighlightFill(_))));
    assert!(same(&base_of(&base), &off_picture), "the off base");
    if !on_gpu {
        assert!(same(&first.picture.unwrap(), &off.picture.unwrap()));
    }

    round = Round::next(round.ahead);
    let next = r
        .go(&on(plain()), 0, &mut base, &mut round, &NoHold)
        .unwrap();
    assert!(!round.gave_way);
    assert!(!next.fills.highlight_pending);
    assert!(!base.as_ref().unwrap().filling);
    assert!(base.as_ref().unwrap().kept.fill().is_some());
    assert!(same(&base_of(&base), &filled_picture), "the one-shot fill");
    if !on_gpu {
        assert!(same(
            next.picture.as_ref().unwrap(),
            filled.picture.as_ref().unwrap()
        ));
    }

    // Kept: nothing pending, the same picture, whatever the turn.
    for turn in [0, 1] {
        let mut again = Round::first();
        let landed = r
            .go(&on(plain()), turn, &mut base, &mut again, &NoHold)
            .unwrap();
        assert!(!landed.fills.highlight_pending, "turn {turn}");
    }
}

#[test]
fn the_first_round_is_the_off_picture_and_the_next_the_filled_one() {
    first_off_then_filled(None);
}

#[test]
fn the_first_round_is_the_off_picture_and_the_next_the_filled_one_on_the_device() {
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        super::tests::skipped("the highlight fill's rounds on the device");
        return;
    };
    first_off_then_filled(Some(ctx));
}

/// With the fill off a first round is what it was: nothing pending,
/// nothing waiting, no fields kept.
#[test]
fn off_is_one_round_as_before() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    let mut base = None;
    let landed = r
        .go(&plain(), 0, &mut base, &mut Round::first(), &NoHold)
        .unwrap();
    assert!(!landed.fills.highlight_pending);
    let b = base.unwrap();
    assert!(!b.filling);
    assert!(b.kept.fill().is_none());
}

/// A hold that gives way before the fill's stage, or inside it at its
/// `inside`th ask (1: after the analysis, 2: after the network, 3:
/// after the synthesis), and counts the asks inside.
struct Stop {
    before: bool,
    inside: usize,
    asked: Cell<usize>,
}

impl Hold for Stop {
    fn give_way(&self, next: &Stage) -> bool {
        self.before && matches!(next, Stage::HighlightFill(_))
    }

    fn give_way_inside(&self, _: &Stage) -> bool {
        self.asked.set(self.asked.get() + 1);
        self.asked.get() == self.inside
    }
}

/// A newer develop stops the fill's round before the fill or between
/// its parts: nothing delivered, no point kept that says filled, no
/// fields kept, the frame's base the off picture it was, and the edit's
/// pictures still waiting; the round after makes the fill whole. A
/// round let run asks three times inside the fill.
#[test]
fn a_superseded_fill_keeps_nothing() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    let counting = Stop {
        before: false,
        inside: usize::MAX,
        asked: Cell::new(0),
    };
    let mut base = None;
    r.go(&on(plain()), 0, &mut base, &mut Round::first(), &NoHold);
    r.go(
        &on(plain()),
        0,
        &mut base,
        &mut Round::next((0, 0)),
        &counting,
    );
    assert_eq!(counting.asked.get(), 3, "between the parts");

    // An edit whose developed picture is its own (the sharpen and the
    // clarity after the base): the frame keeps it, and keeps it still.
    let edit = sharp();
    let (_, filled_base) = r.one_shot(&edit);
    let filled = filled_base.kept.picture(Keep::Base).unwrap().clone();
    for (before, inside) in [(true, 0), (false, 1), (false, 2), (false, 3)] {
        let mut base = None;
        let mut round = Round::first();
        r.go(&edit, 0, &mut base, &mut round, &NoHold);
        let b = base.as_ref().unwrap();
        let was = frame_of(b);
        assert!(b.kept.holds(Keep::Developed));
        let stop = Stop {
            before,
            inside,
            asked: Cell::new(0),
        };
        let mut round = Round::next(round.ahead);
        let landed = r.go(&edit, 0, &mut base, &mut round, &stop);
        assert!(landed.is_none() && round.gave_way, "{before} {inside}");
        let b = base.as_ref().unwrap();
        assert_eq!(frame_of(b), was, "{before} {inside}");
        assert!(b.kept.fill().is_none());
        assert!(!b.kept.made_by(|s| matches!(s, Stage::HighlightFill(_))));
        assert!(b.filling, "still waiting");
        let mut round = Round::next(round.ahead);
        r.go(&edit, 0, &mut base, &mut round, &NoHold).unwrap();
        assert!(same(&base_of(&base), &filled), "{before} {inside}");
    }
}

/// An edit with the sharpen (on by default) and the clarity after its
/// base: its developed picture is a picture of its own.
fn sharp() -> Edit {
    let mut edit = on(Edit::default());
    edit.detail.enabled = true;
    edit.detail.clarity = 0.4;
    edit
}

/// What a frame holds, as one value to compare: its kept points (by
/// keep), their bytes, the base's and the frame's pictures by address,
/// the fields, what it waits on, and what it says it shows.
fn frame_of(b: &FrameKept) -> String {
    let keeps: Vec<Keep> = [
        Keep::Camera,
        Keep::Pair,
        Keep::Base,
        Keep::Patched,
        Keep::PreSharpen,
        Keep::Developed,
    ]
    .into_iter()
    .filter(|k| b.kept.holds(*k))
    .collect();
    format!(
        "{keeps:?} {} base {:?} image {:p} planes {:?} guide {:p} fill {} filling {} edit {:?} \
         stamp {} print {} said {:?}",
        b.kept.bytes(),
        b.kept.picture(Keep::Base).map(Arc::as_ptr),
        Arc::as_ptr(&b.image),
        b.planes.as_ref().map(Arc::as_ptr),
        Arc::as_ptr(&b.guide),
        b.kept.fill().is_some(),
        b.filling,
        b.edit.highlight_fill,
        b.stamp,
        b.print.hash,
        b.developed_as
            .as_ref()
            .map(|(e, t)| (e.highlight_fill.clone(), *t)),
    )
}

/// A walk of the fill's round that fails at any step leaves the frame
/// all as it was (failed before the fill was made) or empty of pictures
/// but for the fields (failed after it), never a mix: and the next
/// develop of the edit makes its base, bumps the stamp, says the edit,
/// and lands the one-shot picture.
#[test]
fn a_fill_round_failed_anywhere_leaves_no_mix() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    let edit = sharp();
    let (one, _) = r.one_shot(&edit);
    let one = one.picture.unwrap();
    let fails = [
        Some("highlight fill"),
        None,
        Some("move into the working space"),
        Some("keep Base"),
        Some("local contrast"),
        Some("keep Patched"),
        Some("sharpen"),
        Some("keep Developed"),
        Some("finish"),
    ];
    for at in fails {
        let mut base = None;
        let mut round = Round::first();
        r.go(&edit, 0, &mut base, &mut round, &NoHold);
        let was = frame_of(base.as_ref().unwrap());
        let stamp = base.as_ref().unwrap().stamp;
        crate::walk::FAIL_AT_BY_TEST.with(|f| *f.borrow_mut() = at.map(String::from));
        crate::walk::FAIL_FILL_BY_TEST.with(|f| f.set(at.is_none()));
        let deliver: Deliver = Arc::new(|_| {});
        let mut round = Round::next(round.ahead);
        let (outcome, _) = develop_round(
            &r.input,
            &edit,
            0,
            1,
            &mut base,
            &mut r.ai,
            (None, None),
            &mut r.gpu,
            &deliver,
            &mut round,
            &NoHold,
        );
        crate::walk::FAIL_AT_BY_TEST.with(|f| *f.borrow_mut() = None);
        crate::walk::FAIL_FILL_BY_TEST.with(|f| f.set(false));
        assert!(matches!(outcome, Outcome::Failed { .. }), "{at:?}");
        let b = base.as_ref().unwrap();
        assert!(!b.filling, "{at:?}");
        let before_fill = matches!(at, None | Some("highlight fill"));
        if before_fill {
            // All as it was but the wait.
            assert_eq!(
                frame_of(b),
                was.replace("filling true", "filling false"),
                "{at:?}"
            );
        } else {
            assert!(b.kept.is_empty(), "{at:?}");
            assert!(b.kept.fill().is_some(), "{at:?}: the fields kept");
            assert!(b.developed_as.is_none(), "{at:?}");
            assert_eq!(b.image.width, 0, "{at:?}");
        }
        let mut round = Round::first();
        let next = r.go(&edit, 0, &mut base, &mut round, &NoHold).unwrap();
        let mut picture = next.picture;
        if next.fills.highlight_pending {
            let mut round = Round::next(round.ahead);
            picture = r
                .go(&edit, 0, &mut base, &mut round, &NoHold)
                .unwrap()
                .picture;
        }
        assert!(same(picture.as_ref().unwrap(), &one), "{at:?}");
        let b = base.as_ref().unwrap();
        assert_eq!(b.edit.highlight_fill.as_deref(), Some("hl-1"), "{at:?}");
        assert!(b.stamp > stamp, "{at:?}");
        assert!(!b.filling);
    }
}

/// The fields kept are counted where the kept pictures are, so the
/// shelf's budget sees them: the filled frame keeps the off frame's
/// bytes and the fields'.
#[test]
fn the_kept_fields_are_counted() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    let (_, off) = r.one_shot(&plain());
    let (_, on) = r.one_shot(&on(plain()));
    let fill = on.kept.fill().expect("fields kept");
    assert!(fill.fill.fields().tiles() > 0);
    assert_eq!(on.kept.bytes(), off.kept.bytes() + fill.bytes() as u64);
    assert_eq!(on.bytes() - off.bytes(), fill.bytes() as u64);
    let mut shelved = on.kept.clone();
    shelved.shelve();
    assert!(shelved.fill().is_some(), "shelved with the base");
}

/// Luminance in stops and chroma (Oklab a, b of the pixel over its
/// luminance) of `p`'s pixels against `q`'s, over `at`: the median and
/// the largest of each.
fn apart(p: &WorkingImage, q: &WorkingImage, at: &[usize]) -> [f64; 4] {
    let mut stops = Vec::new();
    let mut ab = Vec::new();
    for &i in at {
        let px = |w: &WorkingImage| [w.data[3 * i], w.data[3 * i + 1], w.data[3 * i + 2]];
        let (a, b) = (px(p), px(q));
        let (ya, yb) = (oklab::luma(a).max(1e-6), oklab::luma(b).max(1e-6));
        stops.push((ya / yb).log2().abs() as f64);
        let (ca, cb) = (oklab::of(a.map(|v| v / ya)), oklab::of(b.map(|v| v / yb)));
        ab.push(((ca[1] - cb[1]).hypot(ca[2] - cb[2])) as f64);
    }
    let mid = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        (v[v.len() / 2], v[v.len() - 1])
    };
    let (sm, sx) = mid(&mut stops);
    let (am, ax) = mid(&mut ab);
    [sm, sx, am, ax]
}

/// Under a white balance moved by 500 K and by 2000 K the first round
/// shows the last fields through the new white: the picture core makes
/// by hand from the new prepare with those fields through its camera,
/// to the bit. The frame keeps nothing of it (its base, its fields, and
/// its points are the last fill's, the edit's pictures wait), the next
/// round makes the fill again, and an export from what the frame keeps
/// is the one-shot picture. The cells do not move with the white. The
/// stand-in is nearer the fill made again than the unfilled picture
/// is, by the figures printed and pinned, on a scene whose fill lifts
/// the frame's top by about a stop.
#[test]
fn the_last_fields_stand_in_under_a_white_balance_move() {
    let (raw, input) = input_at(900.0);
    let mut r = Rounds::new(&input, None);
    let start = on(at_kelvin(plain(), 5000.0));
    for dk in [500.0, 2000.0] {
        let mut base = None;
        r.go(&start, 0, &mut base, &mut Round::first(), &NoHold);
        r.go(&start, 0, &mut base, &mut Round::next((0, 0)), &NoHold)
            .unwrap();
        let kept_fill = base.as_ref().unwrap().kept.fill().unwrap().clone();
        let kept_base = base_of(&base);

        let moved = at_kelvin(on(plain()), 5000.0 + dk);
        let mut round = Round::first();
        let stand = r.go(&moved, 0, &mut base, &mut round, &NoHold).unwrap();
        assert!(stand.fills.highlight_pending);
        let stand = stand.picture.unwrap();
        let b = base.as_ref().unwrap();
        assert!(b.filling);
        assert!(Arc::ptr_eq(b.kept.fill().unwrap(), &kept_fill));
        assert!(Arc::ptr_eq(&base_of(&base), &kept_base));
        assert!(!b.kept.made_by(|s| matches!(s, Stage::HighlightStandIn(_))));
        assert!(
            b.kept.holds(Keep::Camera),
            "the camera picture for the fill"
        );

        // By hand: the new prepare, its demosaic, the kept fields
        // through its camera, the move into the working space.
        let steps = stages(&moved, &Purpose::Viewport, &input.walked().facts(0), None);
        let Some(Stage::Prepare(p)) = steps.ops().next() else {
            unreachable!()
        };
        assert!(!steps.ops().any(|s| matches!(s, Stage::Denoise(_))));
        // The CA's stage run inside core's prepare, as the walk's
        // stage runs it.
        let settings = DevelopSettings {
            chromatic_aberration: steps.ops().find_map(|s| match s {
                Stage::Ca(o) => Some(*o),
                _ => None,
            }),
            ..p.settings.clone()
        };
        let prepared = prepare_with(&raw, &settings, false, None).unwrap();
        let codes = prepared.clip_codes.clone().unwrap();
        let first = prepare_with(
            &raw,
            &stages(&start, &Purpose::Viewport, &input.walked().facts(0), None)
                .ops()
                .find_map(|s| match s {
                    Stage::Prepare(p) => Some(p.settings.clone()),
                    _ => None,
                })
                .unwrap(),
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            first.clip_codes.as_ref().unwrap().codes,
            codes.codes,
            "the cells do not move with the white"
        );
        assert_ne!(
            first.white_balance.matrix_f32(),
            prepared.white_balance.matrix_f32()
        );
        let (mut rgb, dual, denoised) = demosaic_prepared(&prepared, &settings).unwrap();
        let frame = FrameCodes::new(&codes, 1).unwrap();
        let through = kept_fill.fill.through(&Camera::of(&prepared)).unwrap();
        through.apply(&mut rgb, &frame);
        let by_hand = finish(prepared, rgb, dual, denoised, &settings).unwrap();
        assert!(same(&by_hand.image, &stand), "{dk} K: the stand-in by hand");

        // The fill made again, and the picture unfilled.
        let mut round = Round::next(round.ahead);
        let refill = r.go(&moved, 0, &mut base, &mut round, &NoHold).unwrap();
        let refill = refill.picture.unwrap();
        assert!(!base.as_ref().unwrap().filling);
        let (unfilled, _) = r.one_shot(&Edit {
            highlight_fill: None,
            ..moved.clone()
        });
        let unfilled = unfilled.picture.unwrap();
        let changed: Vec<usize> = (0..stand.data.len() / 3)
            .filter(|&i| {
                (0..3).any(|c| {
                    let u = unfilled.data[3 * i + c].to_bits();
                    stand.data[3 * i + c].to_bits() != u || refill.data[3 * i + c].to_bits() != u
                })
            })
            .collect();
        assert!(changed.len() > 100);
        let si = apart(&stand, &refill, &changed);
        let un = apart(&unfilled, &refill, &changed);
        println!(
            "{dk} K over {} pixels: the stand-in from the fill, median {:.4} stop, largest {:.4}; \
             ab median {:.4}, largest {:.4}. Unfilled: {:.4} stop, largest {:.4}; ab {:.4}, {:.4}",
            changed.len(),
            si[0],
            si[1],
            si[2],
            si[3],
            un[0],
            un[1],
            un[2],
            un[3]
        );
        assert!(
            si[0] < un[0] && si[1] < un[1] && si[2] < un[2],
            "{dk} K: {si:?} {un:?}"
        );
        // Measured: at 500 K the median 0.010 stop, the largest 0.024,
        // ab 0.002 and 0.016; at 2000 K 0.041 and 0.104, ab 0.005 and
        // 0.043. The unfilled picture is 0.15 to 0.18 stop off at the
        // median and 1.1 to 1.2 at the largest.
        let bound = if dk < 1000.0 {
            [0.02, 0.05, 0.005, 0.03]
        } else {
            [0.06, 0.15, 0.01, 0.07]
        };
        assert!(si.iter().zip(bound).all(|(v, b)| *v < b), "{dk} K: {si:?}");

        // An export from what the frame keeps: the one-shot picture.
        let (one, _) = r.one_shot(&moved);
        assert!(same(&refill, one.picture.as_ref().unwrap()));
    }
}

/// The stand-in is barred where the fields are not the frame's under
/// another white: a changed demosaic shows the unfilled picture first,
/// the fields kept as they were. A changed denoise strength is a white
/// balance's kind of move, and the fields stand in.
#[test]
fn the_stand_in_is_barred_across_a_demosaic_and_taken_across_the_denoise() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    let mut denoised = plain();
    denoised.noise.profiled = true;
    denoised.noise.strength = 0.3;
    for (from, to, stands) in [
        (
            on(plain()),
            {
                let mut e = on(plain());
                e.demosaic = greycard_edit::Demosaic::Rcd;
                e
            },
            false,
        ),
        (
            on(denoised.clone()),
            {
                let mut e = on(denoised.clone());
                e.noise.strength = 0.7;
                e
            },
            true,
        ),
    ] {
        let mut base = None;
        r.go(&from, 0, &mut base, &mut Round::first(), &NoHold);
        r.go(&from, 0, &mut base, &mut Round::next((0, 0)), &NoHold)
            .unwrap();
        let fill = base.as_ref().unwrap().kept.fill().unwrap().clone();
        let first = r
            .go(&to, 0, &mut base, &mut Round::first(), &NoHold)
            .unwrap();
        assert!(first.fills.highlight_pending);
        let (unfilled, _) = r.one_shot(&Edit {
            highlight_fill: None,
            ..to.clone()
        });
        assert_eq!(
            same(
                first.picture.as_ref().unwrap(),
                unfilled.picture.as_ref().unwrap()
            ),
            !stands,
            "{stands}"
        );
        assert!(Arc::ptr_eq(
            base.as_ref().unwrap().kept.fill().unwrap(),
            &fill
        ));
    }
}

/// A move after the fill that makes the base again (the defringe here,
/// as the lens's sliders) applies the kept fields again rather than
/// make them: one round, nothing pending, the one-shot picture to the
/// bit.
#[test]
fn a_move_after_the_fill_applies_the_kept_fields_again() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    let mut base = None;
    r.go(&on(plain()), 0, &mut base, &mut Round::first(), &NoHold);
    r.go(
        &on(plain()),
        0,
        &mut base,
        &mut Round::next((0, 0)),
        &NoHold,
    )
    .unwrap();
    let fill = base.as_ref().unwrap().kept.fill().unwrap().clone();
    let mut fringed = on(plain());
    fringed.lens.defringe = true;
    let landed = r
        .go(&fringed, 0, &mut base, &mut Round::first(), &NoHold)
        .unwrap();
    assert!(!landed.fills.highlight_pending);
    assert!(Arc::ptr_eq(
        base.as_ref().unwrap().kept.fill().unwrap(),
        &fill
    ));
    let (one, _) = r.one_shot(&fringed);
    assert!(same(
        landed.picture.as_ref().unwrap(),
        one.picture.as_ref().unwrap()
    ));
}

/// The worker on its thread: an open with the fill on delivers its
/// generation twice, the picture first and the fill after; with it off,
/// once.
#[test]
fn the_worker_delivers_the_fill_after_the_picture() {
    let raw = clipped_raw(160, 120, 2000.0);
    let dir = crate::testing::scratch_dir("highlight-rounds");
    let path = dir.join("lamp.dng");
    let Samples::U16(s) = &raw.samples else {
        unreachable!("the scene is 16-bit")
    };
    let samples: Vec<f32> = s
        .iter()
        .map(|&v| ((v as f32 - 64.0) / (2000.0 - 64.0)).clamp(0.0, 1.0))
        .collect();
    let mut file = std::fs::File::create(&path).unwrap();
    greycard_core::dng::write_cfa_dng(
        &mut file,
        &raw,
        &greycard_core::dng::CfaMosaic {
            samples: &samples,
            width: raw.width,
            height: raw.height,
            pattern: &CfaPattern::rggb(),
        },
        [2.0, 1.0, 1.6],
        &Default::default(),
    )
    .unwrap();
    drop(file);
    for (generation, fill) in [(1u64, true), (2, false)] {
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let worker = Worker::new(move |o| {
            if let Outcome::Developed {
                generation, fills, ..
            } = o
            {
                tx.lock()
                    .unwrap()
                    .send((generation, fills.highlight_pending))
                    .unwrap();
            }
        });
        let mut edit = plain();
        if fill {
            edit = on(edit);
        }
        worker.send(Job::Open {
            path: path.clone(),
            version: None,
            edit,
            generation,
            seed_blend: false,
            turn: 0,
            stance_known: false,
            learned: Vec::new(),
        });
        let wait = std::time::Duration::from_secs(60);
        let first = rx.recv_timeout(wait).expect("the picture");
        assert_eq!(first, (generation, fill));
        if fill {
            assert_eq!(
                rx.recv_timeout(wait).expect("the fill"),
                (generation, false)
            );
        }
        let quiet = std::time::Duration::from_secs(2);
        assert!(rx.recv_timeout(quiet).is_err(), "nothing more");
        worker.stop();
    }
    crate::testing::remove_dir_retry(&dir);
}

/// Which lists' fields may stand in for which: across the white
/// balance and the profiled denoise's strength, not across the
/// demosaic, the denoise turned on, or a list without the fill.
#[test]
fn what_may_stand_in() {
    let (_, input) = input();
    let facts = input.walked().facts(0);
    let through = |e: &Edit| {
        let s = stages(e, &Purpose::Viewport, &facts, None);
        s.fill_at().map(|at| s.steps[..=at].to_vec())
    };
    let from = on(at_kelvin(plain(), 5000.0));
    let kept = through(&from).unwrap();
    let may = |e: Edit| through(&e).is_some_and(|now| crate::stages::stands_in_for(&kept, &now));
    assert!(may(from.clone()));
    assert!(may(at_kelvin(from.clone(), 7000.0)));
    let mut rcd = from.clone();
    rcd.demosaic = greycard_edit::Demosaic::Rcd;
    assert!(!may(rcd));
    let mut denoised = from.clone();
    denoised.noise.profiled = true;
    assert!(!may(denoised));
    // Across the profiled denoise, its strength alone: any other of its
    // settings, the chroma's here, is another picture.
    let mut denoised = from.clone();
    denoised.noise.profiled = true;
    let kept = through(&denoised).unwrap();
    let mut stronger = denoised.clone();
    stronger.noise.strength = 0.9;
    let now = through(&stronger).unwrap();
    assert!(crate::stages::stands_in_for(&kept, &now));
    let chroma = |steps: &mut Vec<crate::stages::Step>| {
        for step in steps.iter_mut() {
            match step {
                crate::stages::Step::Op(Stage::Denoise(o)) => o.chroma += 0.25,
                crate::stages::Step::Op(Stage::Prepare(p)) => {
                    if let Some(o) = p.settings.denoise.as_mut() {
                        o.chroma += 0.25;
                    }
                }
                _ => {}
            }
        }
    };
    let mut other = now.clone();
    chroma(&mut other);
    assert!(!crate::stages::stands_in_for(&kept, &other));
    // In the denoise's step alone, as in the prepare's alone.
    let mut step_only = now.clone();
    for step in step_only.iter_mut() {
        if let crate::stages::Step::Op(Stage::Denoise(o)) = step {
            o.chroma += 0.25;
        }
    }
    assert!(!crate::stages::stands_in_for(&kept, &step_only));
    assert!(!may(Edit {
        highlight_fill: None,
        ..from.clone()
    }));
}

/// Between the rounds the frame says what it shows, the fill-off
/// edit's picture: the edit its masks see, their print (the masks'
/// cache key on disk), and the develop it last delivered are the
/// fill-off edit's; it keeps the camera picture before the fill, and
/// nothing else of its own. After the fill they are the edit's, and the
/// camera picture is let go.
#[test]
fn the_frame_says_off_between_the_rounds_and_on_after() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    let off_print = r.ai.base_print(&plain(), 0, crate::mask_base::Size::Full);
    let on_print =
        r.ai.base_print(&on(plain()), 0, crate::mask_base::Size::Full);
    assert!(off_print != on_print);
    let (_, off) = r.one_shot(&plain());
    let mut base = None;
    let mut round = Round::first();
    r.go(&on(plain()), 0, &mut base, &mut round, &NoHold)
        .unwrap();
    let b = base.as_ref().unwrap();
    assert!(b.edit.highlight_fill.is_none());
    assert!(b.print == off_print);
    let (said, _) = b.developed_as.clone().expect("developed");
    assert!(said.highlight_fill.is_none());
    assert!(b.kept.holds(Keep::Camera));
    // The camera picture: its RGB and its cells, a byte a 2x2 cell.
    assert_eq!(
        b.kept.bytes(),
        off.kept.bytes() + (320 * 240 * 3 * 4 + 160 * 120) as u64
    );

    round = Round::next(round.ahead);
    r.go(&on(plain()), 0, &mut base, &mut round, &NoHold)
        .unwrap();
    let b = base.as_ref().unwrap();
    assert_eq!(b.edit.highlight_fill.as_deref(), Some("hl-1"));
    assert!(b.print == on_print);
    let (said, _) = b.developed_as.clone().expect("developed");
    assert_eq!(said.highlight_fill.as_deref(), Some("hl-1"));
    assert!(!b.kept.holds(Keep::Camera));
}

/// A fill round that fails, not given way, leaves the edit's pictures
/// nothing to wait on and the frame's kept points as they were.
#[test]
fn a_failed_fill_round_waits_on_nothing() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    let mut base = None;
    let mut round = Round::first();
    r.go(&on(plain()), 0, &mut base, &mut round, &NoHold)
        .unwrap();
    let was = base_of(&base);
    let bytes = base.as_ref().unwrap().kept.bytes();
    assert!(base.as_ref().unwrap().filling);
    crate::walk::FAIL_FILL_BY_TEST.with(|f| f.set(true));
    let deliver: Deliver = Arc::new(|_| {});
    let mut round = Round::next(round.ahead);
    let (outcome, _) = develop_round(
        &r.input,
        &on(plain()),
        0,
        1,
        &mut base,
        &mut r.ai,
        (None, None),
        &mut r.gpu,
        &deliver,
        &mut round,
        &NoHold,
    );
    crate::walk::FAIL_FILL_BY_TEST.with(|f| f.set(false));
    assert!(matches!(outcome, Outcome::Failed { .. }) && !round.gave_way);
    let b = base.as_ref().unwrap();
    assert!(!b.filling, "nothing to wait on");
    assert!(Arc::ptr_eq(&base_of(&base), &was));
    assert_eq!(b.kept.bytes(), bytes);
}

/// The fields go with the last filled picture: a frame whose fill was
/// turned off shelves none; one filled shelves them.
#[test]
fn a_frame_with_its_fill_off_shelves_no_fields() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    let mut base = None;
    r.go(&on(plain()), 0, &mut base, &mut Round::first(), &NoHold);
    r.go(
        &on(plain()),
        0,
        &mut base,
        &mut Round::next((0, 0)),
        &NoHold,
    )
    .unwrap();
    let mut filled = base.as_ref().unwrap().kept.clone();
    filled.shelve();
    assert!(filled.fill().is_some());
    r.go(&plain(), 0, &mut base, &mut Round::first(), &NoHold)
        .unwrap();
    let mut off = base.as_ref().unwrap().kept.clone();
    assert!(off.fill().is_some(), "the open frame keeps them");
    off.shelve();
    assert!(off.fill().is_none());
    assert!(!off.holds(Keep::Camera));
}

/// The worker on its thread: a develop sent while the fill's round
/// runs stops it, so the open's filled picture never lands and the
/// newer develop's rounds take over. Without the round's hold the
/// open's fill would land first.
#[test]
fn a_newer_develop_stops_the_fill_round() {
    let raw = clipped_raw(160, 120, 2000.0);
    let dir = crate::testing::scratch_dir("highlight-newer");
    let path = dir.join("lamp.dng");
    let Samples::U16(s) = &raw.samples else {
        unreachable!("the scene is 16-bit")
    };
    let samples: Vec<f32> = s
        .iter()
        .map(|&v| ((v as f32 - 64.0) / (2000.0 - 64.0)).clamp(0.0, 1.0))
        .collect();
    let mut file = std::fs::File::create(&path).unwrap();
    greycard_core::dng::write_cfa_dng(
        &mut file,
        &raw,
        &greycard_core::dng::CfaMosaic {
            samples: &samples,
            width: raw.width,
            height: raw.height,
            pattern: &CfaPattern::rggb(),
        },
        [2.0, 1.0, 1.6],
        &Default::default(),
    )
    .unwrap();
    drop(file);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let worker = Worker::new(move |o| {
        if let Outcome::Developed {
            generation, fills, ..
        } = o
        {
            tx.lock()
                .unwrap()
                .send((generation, fills.highlight_pending))
                .unwrap();
        }
    });
    let edit = on(plain());
    worker.queue.0.lock().unwrap().develop_on_fill_ask = Some(Job::Develop {
        edit: edit.clone(),
        generation: 2,
        turn: 0,
        of: None,
    });
    worker.send(Job::Open {
        path: path.clone(),
        version: None,
        edit,
        generation: 1,
        seed_blend: false,
        turn: 0,
        stance_known: false,
        learned: Vec::new(),
    });
    let wait = std::time::Duration::from_secs(60);
    let said: Vec<(u64, bool)> = (0..3)
        .map(|_| rx.recv_timeout(wait).expect("a delivery"))
        .collect();
    assert_eq!(said, [(1, true), (2, true), (2, false)]);
    assert!(
        rx.recv_timeout(std::time::Duration::from_secs(2)).is_err(),
        "nothing more"
    );
    worker.stop();
    crate::testing::remove_dir_retry(&dir);
}

/// Wall time at 24 MP, the window's rounds against one walk: the
/// fill-off develop, one walk with the fill, the first round and the
/// fill landed after it, and under a white balance move the stand-in
/// and the fill made again after it. On the device with
/// `GREYCARD_TIMING_GPU` set. A measurement, printed: run in release.
#[test]
#[ignore = "a measurement, printed: run in release"]
fn the_rounds_timed_at_24_megapixels() {
    let raw = Arc::new(clipped_raw(6000, 4000, 2000.0));
    let input = Input::Raw(raw);
    let gpu = std::env::var_os("GREYCARD_TIMING_GPU")
        .map(|_| greycard_gpu::Context::own().expect("a device"));
    let on_gpu = gpu.is_some();
    let mut r = Rounds::new(&input, gpu);
    r.go(&plain(), 0, &mut None, &mut Round::first(), &NoHold);
    for _ in 0..2 {
        let timed = |r: &mut Rounds, f: &mut dyn FnMut(&mut Rounds)| {
            let t = Instant::now();
            f(r);
            t.elapsed().as_secs_f64()
        };
        let off = timed(&mut r, &mut |r| {
            r.go(&plain(), 0, &mut None, &mut Round::first(), &NoHold);
        });
        let one = timed(&mut r, &mut |r| {
            r.go(&on(plain()), 0, &mut None, &mut Round::all(), &NoHold);
        });
        let mut b = None;
        let first = timed(&mut r, &mut |r| {
            r.go(&on(plain()), 0, &mut b, &mut Round::first(), &NoHold);
        });
        let filled = timed(&mut r, &mut |r| {
            r.go(&on(plain()), 0, &mut b, &mut Round::next((0, 0)), &NoHold);
        });
        let moved = on(at_kelvin(plain(), 6500.0));
        let stand = timed(&mut r, &mut |r| {
            r.go(&moved, 0, &mut b, &mut Round::first(), &NoHold);
        });
        let refill = timed(&mut r, &mut |r| {
            r.go(&moved, 0, &mut b, &mut Round::next((0, 0)), &NoHold);
        });
        println!(
            "timed, device {on_gpu}, {} threads: off {off:.2} s; one walk {one:.2} s; \
             rounds: first {first:.2} s, filled {:.2} s after it ({:.2} s in all); \
             white balance: stand-in {stand:.2} s, filled {refill:.2} s after it",
            rayon::current_num_threads(),
            filled,
            first + filled,
        );
    }
}

/// A white balance moved while the fill is still pending (no fields
/// yet, the camera picture kept at the old white): the first round of
/// the new white is the new white's picture without the fill, and its
/// fill round the new white's filled one; the old camera picture
/// serves neither.
#[test]
fn a_white_balance_moved_while_the_fill_is_pending() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    let a = on(at_kelvin(plain(), 5000.0));
    let b = on(at_kelvin(plain(), 6500.0));
    let (off_b, _) = r.one_shot(&Edit {
        highlight_fill: None,
        ..b.clone()
    });
    let (on_b, _) = r.one_shot(&b);
    let mut base = None;
    let mut round = Round::first();
    r.go(&a, 0, &mut base, &mut round, &NoHold).unwrap();
    assert!(base.as_ref().unwrap().kept.holds(Keep::Camera));
    let mut round = Round::first();
    let first = r.go(&b, 0, &mut base, &mut round, &NoHold).unwrap();
    assert!(first.fills.highlight_pending);
    assert!(
        same(
            first.picture.as_ref().unwrap(),
            off_b.picture.as_ref().unwrap()
        ),
        "first round"
    );
    let mut round = Round::next(round.ahead);
    let next = r.go(&b, 0, &mut base, &mut round, &NoHold).unwrap();
    assert!(
        same(
            next.picture.as_ref().unwrap(),
            on_b.picture.as_ref().unwrap()
        ),
        "fill round"
    );
}

/// The fill's round, as the worker runs it, lets go of every picture
/// the frame holds without the fill once the fill is made and before
/// the filled base is: the base, its kept points, the masks' bands, and
/// the guide. The picture without the fill and the filled one are never
/// held at once.
#[test]
fn the_round_lets_go_of_the_picture_without_the_fill() {
    let (_, input) = input();
    let mut r = Rounds::new(&input, None);
    for edit in [on(plain()), sharp()] {
        let mut base = None;
        let mut round = Round::first();
        r.go(&edit, 0, &mut base, &mut round, &NoHold).unwrap();
        let b = base.as_ref().unwrap();
        let mut held: Vec<std::sync::Weak<WorkingImage>> = vec![
            Arc::downgrade(&b.image),
            Arc::downgrade(b.kept.picture(Keep::Base).unwrap()),
        ];
        held.extend(b.kept.picture(Keep::Developed).map(Arc::downgrade));
        let planes = b.planes.as_ref().map(Arc::downgrade);
        let guide = Arc::downgrade(&b.guide);
        let seen = Arc::new(std::sync::Mutex::new(None));
        let said = seen.clone();
        crate::walk::ON_PAST_BY_TEST.with(|f| {
            *f.borrow_mut() = Some(Box::new(move || {
                let alive = held.iter().filter(|w| w.upgrade().is_some()).count()
                    + planes.iter().filter(|w| w.upgrade().is_some()).count()
                    + usize::from(guide.upgrade().is_some());
                *said.lock().unwrap() = Some(alive);
            }));
        });
        let mut round = Round::next(round.ahead);
        r.go(&edit, 0, &mut base, &mut round, &NoHold).unwrap();
        crate::walk::ON_PAST_BY_TEST.with(|f| *f.borrow_mut() = None);
        assert_eq!(*seen.lock().unwrap(), Some(0), "{:?}", edit.detail);
    }
}

/// The variable a measurement's own process reads: a long edge, in
/// pixels, for [`round_memory_measured`] to measure at.
const ROUND_MEMORY: &str = "GREYCARD_ROUND_MEMORY";

/// The fill's rounds' memory as the worker's develop runs them, in a
/// process of its own (the count is the whole process's, by this
/// binary's counting allocator), at the long
/// edge `GREYCARD_ROUND_MEMORY` names, 3:2, for an edit with the sharpen
/// off and one with the sharpen and the clarity: the fill-off develop's
/// peak, one walk with the fill, the first round, and the fill round's
/// over what the frame held when it began; and what was live when the
/// round let go of the picture without the fill, over the same. A line
/// each on stdout, in bytes. Nothing without the variable.
#[test]
#[ignore = "run in a process of its own by the_rounds_memory_is_pinned, or by hand in release"]
fn round_memory_measured() {
    let Some(long) = std::env::var(ROUND_MEMORY)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    else {
        return;
    };
    let raw = Arc::new(clipped_raw(long, long * 2 / 3, 2000.0));
    let input = Input::Raw(raw);
    let mut r = Rounds::new(&input, None);
    // The thread pool and the profile caches, made before the counts.
    r.go(&plain(), 0, &mut None, &mut Round::first(), &NoHold);
    COUNTING.store(true, std::sync::atomic::Ordering::SeqCst);
    for (label, edit) in [("plain", on(plain())), ("sharp", sharp())] {
        let off_edit = Edit {
            highlight_fill: None,
            ..edit.clone()
        };
        let (landed, off) = peak_of(|| r.go(&off_edit, 0, &mut None, &mut Round::first(), &NoHold));
        drop(landed);
        let (landed, one) = peak_of(|| r.go(&edit, 0, &mut None, &mut Round::all(), &NoHold));
        let picture = landed.unwrap().picture.unwrap().data.len() * 4;
        let mut base = None;
        let mut round = Round::first();
        let (landed, first) = peak_of(|| r.go(&edit, 0, &mut base, &mut round, &NoHold));
        drop(landed);
        let held = live();
        let at_past = Arc::new(std::sync::atomic::AtomicIsize::new(isize::MIN));
        let seen = at_past.clone();
        crate::walk::ON_PAST_BY_TEST.with(|f| {
            *f.borrow_mut() = Some(Box::new(move || {
                seen.store(live(), std::sync::atomic::Ordering::SeqCst)
            }));
        });
        let mut round = Round::next(round.ahead);
        let (landed, fill_round) = peak_of(|| r.go(&edit, 0, &mut base, &mut round, &NoHold));
        crate::walk::ON_PAST_BY_TEST.with(|f| *f.borrow_mut() = None);
        drop(landed);
        drop(base);
        println!(
            "ROUND_MEMORY {label} picture {picture} off {off} one {one} first {first} \
             round {fill_round} at_past {}",
            at_past.load(std::sync::atomic::Ordering::SeqCst) - held
        );
    }
}

/// The fill's rounds' memory, measured in a process of its own at 1200
/// by 800 through the worker's develop: the fill round holds less than
/// the first round and than one walk with the fill (no mosaic, no
/// demosaic), for the edit with the sharpen off at most two and a half
/// pictures over what the frame held when it began (the camera
/// picture's copy the fill works on, and the fill's analysis, or the
/// filled base and the finish), and when it lets go of the picture without the
/// fill it holds little more than its own working copy: the picture
/// without the fill gone, base, bands, and all.
#[test]
fn the_rounds_memory_is_pinned() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "worker::highlight_rounds::round_memory_measured",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROUND_MEMORY, "1200")
        .output()
        .expect("the measurement runs");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{said}");
    // The first follows the harness's own words on its line.
    let lines: Vec<&str> = said
        .lines()
        .filter_map(|l| l.find("ROUND_MEMORY").map(|i| &l[i..]))
        .collect();
    assert_eq!(lines.len(), 2, "{said}");
    for line in lines {
        let words: Vec<&str> = line.split_whitespace().collect();
        let of = |key: &str| -> f64 {
            let i = words.iter().position(|w| *w == key).expect(key);
            words[i + 1].parse().unwrap()
        };
        let p = of("picture");
        let (one, first, round, past) = (
            of("one") / p,
            of("first") / p,
            of("round") / p,
            of("at_past") / p,
        );
        eprintln!("{line}: one {one:.2}, first {first:.2}, round {round:.2}, let go {past:.2}");
        assert!(round < first && round < one, "{line}");
        if line.contains(" plain ") {
            assert!(round < 2.5, "{line}");
        }
        assert!(past < 0.6, "{line}: the picture without the fill let go");
    }
}

/// This binary's allocator: the system's, counting what is live and the
/// most since a mark while a measurement asks it to (one relaxed load
/// a call otherwise).
struct Counting;

static COUNTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LIVE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static PEAK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn grew(by: isize) {
    use std::sync::atomic::Ordering::{Relaxed, SeqCst};
    if COUNTING.load(Relaxed) {
        let now = LIVE.fetch_add(by, SeqCst) + by;
        PEAK.fetch_max(now, SeqCst);
    }
}

fn live() -> isize {
    LIVE.load(std::sync::atomic::Ordering::SeqCst)
}

/// The most `f` held at once over what was live before it.
fn peak_of<T>(f: impl FnOnce() -> T) -> (T, isize) {
    let before = live();
    PEAK.store(before, std::sync::atomic::Ordering::SeqCst);
    let out = f();
    (out, PEAK.load(std::sync::atomic::Ordering::SeqCst) - before)
}

// SAFETY: every call is the system allocator's, passed on; the counts
// are bookkeeping beside it.
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { std::alloc::System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size() as isize);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: the caller's contract, passed on.
        unsafe { std::alloc::System.dealloc(ptr, layout) };
        grew(-(layout.size() as isize));
    }

    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { std::alloc::System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size() as isize);
        }
        p
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { std::alloc::System.realloc(ptr, layout, size) };
        if !p.is_null() {
            grew(size as isize - layout.size() as isize);
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;
