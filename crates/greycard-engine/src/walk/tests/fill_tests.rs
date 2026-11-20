//! The learned highlight fill's stage in the walk (notes §719 §1a).

use greycard_core::develop::clipcodes::ClipCodes;
use greycard_core::develop::highlight_fill::half::PixelCodes;
use greycard_core::develop::highlight_fill::{BinnedCodes, Camera, FrameCodes, camera_fields};
use greycard_core::raw::{CfaPattern, LevelPattern, Samples, SensorLayout};

use super::*;
use crate::stages::{HIGHLIGHT_FILL, NO_SUCH_FILL, Skip};

/// [`frame_of`]'s scene with its white at 2000: the right of the ramp
/// and the disk clip on green, the brightest of the disk on red too.
fn clipped_frame_of(w: usize, h: usize) -> Arc<RawFrame> {
    clipped_with(w, h, 2000.0)
}

/// [`frame_of`]'s scene with its white at `white`.
fn clipped_with(w: usize, h: usize, white: f32) -> Arc<RawFrame> {
    let mut raw = (*frame_of(w, h, [1.0, 1.0])).clone();
    raw.levels.white = LevelPattern::uniform(white, 1);
    Arc::new(raw)
}

fn on(mut edit: Edit) -> Edit {
    edit.highlight_fill = Some("hl-1".into());
    edit
}

/// An edit that leaves the base as the move into the working space
/// makes it: nothing after it moves a pixel before the base.
fn plain() -> Edit {
    let mut edit = Edit::default();
    edit.sharpen.enabled = false;
    edit
}

fn base(kept: &Kept) -> Arc<WorkingImage> {
    kept.picture(Keep::Base).cloned().expect("a base")
}

/// The prepare's settings in `steps`, with the CA core's own prepare
/// runs inside it, as the walk runs it in a stage of its own.
fn by_hand_settings(steps: &Steps) -> DevelopSettings {
    let Some(Stage::Prepare(prepare)) = steps.ops().next() else {
        unreachable!("a raw's list starts at its prepare")
    };
    let ca = steps.ops().find_map(|s| match s {
        Stage::Ca(o) => Some(*o),
        _ => None,
    });
    DevelopSettings {
        chromatic_aberration: ca,
        ..prepare.settings.clone()
    }
}

/// The pixels where `a` and `b` differ in any channel's bits.
fn moved(a: &WorkingImage, b: &WorkingImage) -> Vec<(usize, usize)> {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let w = a.width;
    a.pixels()
        .zip(b.pixels())
        .enumerate()
        .filter(|(_, (p, q))| p.map(f32::to_bits) != q.map(f32::to_bits))
        .map(|(i, _)| (i % w, i / w))
        .collect()
}

/// The fill on moves only pixels whose cell clipped, as many as it
/// says it changed, and the picture is core's fill run by hand: in
/// camera space to the bit, and through the working space (the fill
/// after the matrix, as the trial ran it) to 2e-6 of the pixel's
/// largest channel, the rounding of the matrix meeting its inverse
/// once each way in f32.
#[test]
fn the_fill_moves_only_clipped_cells_and_is_cores_by_hand() {
    let raw = clipped_frame_of(320, 240);
    let input = Input::Raw(raw.clone());
    let mut off_kept = Kept::new();
    let off = walk_cpu(&plain(), &Purpose::Viewport, &input, 0, &mut off_kept);
    let mut on_kept = Kept::new();
    let walked = walk_cpu(&on(plain()), &Purpose::Viewport, &input, 0, &mut on_kept);
    assert!(off.filled.is_none());
    let filled = walked.filled.clone().expect("the fill ran");
    assert_eq!(filled.version, "hl-1");
    assert_eq!(filled.look_scale, 1.0);
    assert!(walked.seconds.iter().any(|(s, _)| *s == "highlight fill"));
    let (off_base, on_base) = (base(&off_kept), base(&on_kept));

    let steps = stages::stages(&on(plain()), &Purpose::Viewport, &input.facts(0), None);
    let settings = by_hand_settings(&steps);
    assert!(settings.clip_codes);
    let prepared = prepare_with(&raw, &settings, false, None).unwrap();
    let codes = prepared.clip_codes.clone().expect("the cells read");
    let moved = moved(&off_base, &on_base);
    assert!(moved.len() > 100, "{} moved", moved.len());
    assert_eq!(moved.len(), filled.changed);
    assert!(moved.iter().all(|&(x, y)| codes.at(x, y) > 0));

    let (w, h) = (prepared.width, prepared.height);
    let (rgb, dual, denoised) = demosaic_prepared(&prepared, &settings).unwrap();
    let version = greycard_ai::highlight::version("hl-1").unwrap();
    let frame_codes = FrameCodes::new(&codes, 1).unwrap();
    let camera = Camera::of(&prepared);
    let fill = camera_fields(
        &rgb,
        w,
        h,
        &camera,
        Some(&frame_codes),
        frame_codes.clips(),
        version.weights(),
        &version.look,
        1.0,
    )
    .unwrap();
    let mut camera_rgb = rgb.clone();
    fill.apply(&mut camera_rgb, &frame_codes);
    let in_camera =
        core_develop::finish(prepared.clone(), camera_rgb, dual, denoised, &settings).unwrap();
    assert!(
        in_camera.image.data == on_base.data,
        "the fill in camera space"
    );

    // After the matrix: the fill's input made whole, filled there.
    let unfilled = core_develop::finish(prepared, rgb, dual, denoised, &settings).unwrap();
    assert!(unfilled.image.data == off_base.data);
    let s = camera.scale();
    let mut x: Vec<f32> = unfilled.image.data.iter().map(|v| v / s).collect();
    let fields = greycard_core::develop::highlight_fill::fill_fields(
        &x,
        w,
        h,
        &frame_codes,
        frame_codes.clips(),
        version.weights(),
        &version.look,
        1.0,
    )
    .unwrap();
    assert_eq!(&fields, fill.fields(), "the same fields to the bit");
    fields.apply(&mut x, &frame_codes);
    let mut worst = 0.0f32;
    for (got, want) in on_base.pixels().zip(x.as_chunks::<3>().0) {
        let top = want.iter().fold(0.0f32, |m, v| m.max(v.abs())) * s;
        for (g, v) in got.iter().zip(want) {
            worst = worst.max((g - v * s).abs() / top.max(1e-6));
        }
    }
    eprintln!("the fill in camera space against after the matrix: {worst:e}");
    assert!(worst < 2e-6, "{worst:e}");
}

/// A frame with nothing clipped makes the same picture with the fill
/// on as off, to the bit, and the same guide.
#[test]
fn nothing_clipped_is_the_same_bits_on() {
    let input = Input::Raw(frame_of(320, 240, [1.0, 1.0]));
    let off = walk_cpu(&plain(), &Purpose::Viewport, &input, 0, &mut Kept::new());
    let on = walk_cpu(
        &on(plain()),
        &Purpose::Viewport,
        &input,
        0,
        &mut Kept::new(),
    );
    assert!(on.filled.is_none());
    assert!(on.meta.range_shift.is_none());
    assert!(picture(&on).data == picture(&off).data);
    assert_eq!(on.taps.guide, off.taps.guide);
}

/// Highlights and Whites read the range from before the fill: with it
/// on, the guide's range is off's to 1e-5 stops (the shift is taken
/// on the picture the tap reads here, so only its subtraction and its
/// sum round), though the plane is the filled picture's and its own
/// range is not.
#[test]
fn the_guide_reads_the_range_from_before_the_fill() {
    let input = Input::Raw(clipped_frame_of(320, 240));
    let off = walk_cpu(&plain(), &Purpose::Viewport, &input, 0, &mut Kept::new());
    let on = walk_cpu(
        &on(plain()),
        &Purpose::Viewport,
        &input,
        0,
        &mut Kept::new(),
    );
    let shift = on.meta.range_shift.expect("the fill moved the range");
    let (off_guide, on_guide) = (
        off.taps.guide.clone().unwrap(),
        on.taps.guide.clone().unwrap(),
    );
    assert!(
        on_guide.data != off_guide.data,
        "the plane is the filled picture's"
    );
    let own = greycard_render::finish::guide_plane(&picture(&on)).range;
    assert!(
        (own[1] - off_guide.range[1]).abs() > 1e-3,
        "{own:?} against {:?}",
        off_guide.range
    );
    eprintln!(
        "range off {:?}, on {:?}, unshifted {own:?}, shift {shift:?}",
        off_guide.range, on_guide.range
    );
    for i in 0..2 {
        assert!(
            (on_guide.range[i] - off_guide.range[i]).abs() < 1e-5,
            "{:?} against {:?} (shift {shift:?})",
            on_guide.range,
            off_guide.range
        );
    }
}

/// A frame with light over its white (§716): the fill runs, and moves
/// only pixels whose cell clipped at its headroom.
#[test]
fn a_frame_with_headroom_fills_its_clipped_cells() {
    let mut raw = (*clipped_frame_of(320, 240)).clone();
    raw.levels.saturation = Some(3000.0);
    assert!(raw.levels.headroom() > 1.2);
    let raw = Arc::new(raw);
    let input = Input::Raw(raw.clone());
    let mut off_kept = Kept::new();
    walk_cpu(&plain(), &Purpose::Viewport, &input, 0, &mut off_kept);
    let mut on_kept = Kept::new();
    let walked = walk_cpu(&on(plain()), &Purpose::Viewport, &input, 0, &mut on_kept);
    let filled = walked.filled.expect("the fill ran");
    let steps = stages::stages(&on(plain()), &Purpose::Viewport, &input.facts(0), None);
    let prepared = prepare_with(&raw, &by_hand_settings(&steps), false, None).unwrap();
    assert!(prepared.headroom > 1.2);
    let codes = prepared.clip_codes.unwrap();
    let moved = moved(&base(&off_kept), &base(&on_kept));
    assert_eq!(moved.len(), filled.changed);
    assert!(!moved.is_empty());
    assert!(moved.iter().all(|&(x, y)| codes.at(x, y) > 0));
}

fn pair(kept: &Kept) -> (Arc<WorkingImage>, Arc<WorkingImage>) {
    match &kept
        .points
        .iter()
        .find(|p| p.keep == Keep::Pair)
        .expect("a pair")
        .buf
    {
        KeptBuf::Pair { model, plain } => (model.clone(), plain.clone()),
        _ => panic!("a pair in memory"),
    }
}

/// The learned pair: the fields made once on the plain half move both
/// halves, each only where a cell clipped; at strength 0 the base is
/// the plain demosaic filled, to the bit; and a turned base takes the
/// filled pair with it.
#[test]
fn the_pair_takes_one_fill_on_both_halves() {
    let raw = clipped_frame_of(160, 120);
    let input = Input::Raw(raw.clone());
    let mut e = plain();
    e.noise.learned = greycard_edit::Learned::Fast;
    let there = || Fake {
        there: true,
        ..Fake::default()
    };
    let mut off_kept = Kept::new();
    walk_with(&e, &Purpose::Viewport, &input, &mut off_kept, &mut there());
    let mut on_kept = Kept::new();
    let walked = walk_with(
        &on(e.clone()),
        &Purpose::Viewport,
        &input,
        &mut on_kept,
        &mut there(),
    );
    assert!(walked.meta.stood_in.is_none());
    assert!(walked.filled.is_some());
    let steps = stages::stages(&on(e.clone()), &Purpose::Viewport, &input.facts(0), None);
    let codes = prepare_with(&raw, &by_hand_settings(&steps), true, None)
        .unwrap()
        .clip_codes
        .unwrap();
    let ((off_model, off_plain), (on_model, on_plain)) = (pair(&off_kept), pair(&on_kept));
    for (what, off, on) in [
        ("network", &off_model, &on_model),
        ("plain", &off_plain, &on_plain),
    ] {
        let moved = moved(off, on);
        assert!(!moved.is_empty(), "{what}");
        assert!(moved.iter().all(|&(x, y)| codes.at(x, y) > 0), "{what}");
    }
    assert_eq!(
        moved(&off_plain, &on_plain).len(),
        walked.filled.unwrap().changed
    );

    // Strength 0: the plain half, which is the engine's own path filled.
    e.noise.learned_strength = 0.0;
    e.noise.learned_chroma_strength = 0.0;
    let mut kept = Kept::new();
    walk_with(
        &on(e.clone()),
        &Purpose::Viewport,
        &input,
        &mut kept,
        &mut there(),
    );
    let mut engine = e.clone();
    engine.noise.learned = greycard_edit::Learned::Off;
    engine.noise.profiled = false;
    let mut engine_kept = Kept::new();
    walk_cpu(&on(engine), &Purpose::Viewport, &input, 0, &mut engine_kept);
    assert!(base(&kept).data == base(&engine_kept).data);

    // The turned base, as `a_turned_base_takes_the_learned_pair_with_it`.
    e.noise.learned_strength = 0.5;
    e.noise.learned_chroma_strength = 0.5;
    let e = on(e);
    let mut kept = Kept::new();
    walk_at(&e, &Purpose::Viewport, &input, 0, &mut kept, &mut there());
    let turned = walk_at(&e, &Purpose::Viewport, &input, 1, &mut kept, &mut there());
    assert_eq!(turned.from, Some(Keep::Base));
    let mut fresh = Kept::new();
    let made = walk_at(&e, &Purpose::Viewport, &input, 1, &mut fresh, &mut there());
    assert!(picture(&turned).data == picture(&made).data);
    let ((model, plain), (fresh_model, fresh_plain)) = (pair(&kept), pair(&fresh));
    assert!(model.data == fresh_model.data && plain.data == fresh_plain.data);
    assert_eq!(turned.filled, made.filled, "carried with the base");
}

/// A DNG's warp moves the filled picture: the walk is the fill in
/// camera space, then the warp, then the move into the working space,
/// run by hand, to the bit.
#[test]
fn a_files_warp_moves_the_filled_picture() {
    let mut frame = (*clipped_frame_of(192, 128)).clone();
    let mut lens = greycard_core::opcodes::WarpPlane::identity();
    (lens.radial[0], lens.radial[2]) = (0.97, 0.06);
    frame.dng.warps = vec![greycard_core::opcodes::Warp {
        planes: vec![lens],
        center_x: 0.48,
        center_y: 0.53,
        reciprocal: false,
    }];
    let raw = Arc::new(frame);
    let input = Input::Raw(raw.clone());
    let edit = on(plain());
    let steps = stages::stages(&edit, &Purpose::Viewport, &input.facts(0), None);
    let names: Vec<&str> = steps.ops().map(Stage::name).collect();
    let at = |n: &str| names.iter().position(|s| *s == n).unwrap();
    assert_eq!(at("highlight fill") + 1, at("the file's warp"));
    let mut kept = Kept::new();
    let walked = walk_cpu(&edit, &Purpose::Viewport, &input, 0, &mut kept);
    assert!(walked.filled.is_some());

    let settings = by_hand_settings(&steps);
    let prepared = prepare_with(&raw, &settings, false, None).unwrap();
    let codes = prepared.clip_codes.clone().unwrap();
    let (w, h) = (prepared.width, prepared.height);
    let (mut rgb, dual, denoised) = demosaic_prepared(&prepared, &settings).unwrap();
    let version = greycard_ai::highlight::version("hl-1").unwrap();
    let frame_codes = FrameCodes::new(&codes, 1).unwrap();
    camera_fields(
        &rgb,
        w,
        h,
        &Camera::of(&prepared),
        Some(&frame_codes),
        frame_codes.clips(),
        version.weights(),
        &version.look,
        1.0,
    )
    .unwrap()
    .apply(&mut rgb, &frame_codes);
    let options = steps
        .ops()
        .find_map(|s| match s {
            Stage::Warp(o) => Some(o.clone()),
            _ => None,
        })
        .unwrap();
    apply_warps(
        &mut rgb,
        w,
        h,
        WarpPlacement::binned(options.area, 1),
        &options.warps,
    );
    let want = core_develop::finish(prepared, rgb, dual, denoised, &settings).unwrap();
    assert!(base(&kept).data == want.image.data);
}

/// The export's list resumes from the viewport's kept points with the
/// fill on, as with it off, and says what the fill did as a fresh walk
/// does.
#[test]
fn an_export_resumes_from_the_viewports_filled_points() {
    let input = Input::Raw(clipped_frame_of(160, 120));
    let edit = on(with_subject(edit()));
    let mut kept = Kept::new();
    walk_cpu(&edit, &Purpose::Viewport, &input, 0, &mut kept);
    let purpose = Purpose::Export(Box::default());
    let mut export = stages::stages(&edit, &purpose, &input.facts(0), None);
    export.steps.retain(|s| !matches!(s, Step::Keep(_)));
    let export_plan = plan(&export, &purpose, false);
    let resumed = walk(
        &export,
        &export_plan,
        &input,
        &mut kept,
        &mut NoModels,
        &mut None,
        &NoHold,
    )
    .unwrap();
    assert_eq!(resumed.from, Some(Keep::Developed));
    let fresh = walk(
        &export,
        &export_plan,
        &input,
        &mut Kept::new(),
        &mut NoModels,
        &mut None,
        &NoHold,
    )
    .unwrap();
    assert!(fresh.filled.is_some());
    assert_eq!(resumed.filled, fresh.filled);
    assert_eq!(resumed.meta.range_shift, fresh.meta.range_shift);
    assert!(picture(&resumed).data == picture(&fresh).data);
}

/// On a device, the viewport, the prefetch, and the export run the
/// demosaic there and the fill on the CPU after it, and make one base
/// to the bit. Run on lavapipe.
#[test]
fn the_fill_follows_the_devices_demosaic_in_every_plan() {
    let Some(mut gpu) = device() else {
        return;
    };
    let input = Input::Raw(clipped_frame_of(160, 120));
    for version in greycard_edit::key::HIGHLIGHT_FILLS {
        let mut e = on(edit());
        e.highlight_fill = Some((*version).into());
        e.lens.defringe = false;
        let go = |purpose: &Purpose, kept: &mut Kept, gpu: &mut Option<greycard_gpu::Context>| {
            let steps = stages::stages(&e, purpose, &input.facts(0), None);
            let plan = plan(&steps, purpose, gpu.is_some());
            let walked = walk(&steps, &plan, &input, kept, &mut NoModels, gpu, &NoHold).unwrap();
            let fill = steps
                .steps
                .iter()
                .position(|s| matches!(s, Step::Op(Stage::HighlightFill(_))))
                .unwrap();
            assert_eq!(walked.places[fill], Place::Cpu);
            assert_eq!(walked.filled.as_ref().map(|f| f.version), Some(*version));
            walked
        };
        let mut viewport = Kept::new();
        let walked = go(&Purpose::Viewport, &mut viewport, &mut gpu);
        assert!(
            walked
                .seconds
                .iter()
                .any(|(name, _)| *name == "demosaic on the GPU")
        );
        let mut prefetch = Kept::new();
        go(&Purpose::Prefetch, &mut prefetch, &mut gpu);
        let mut export = Kept::new();
        go(&Purpose::Export(Box::default()), &mut export, &mut gpu);
        let b = |k: &Kept| k.picture(Keep::Base).cloned().unwrap();
        assert!(b(&viewport).data == b(&prefetch).data, "{version}");
        assert!(b(&viewport).data == b(&export).data, "{version}");
    }
}

/// hl-2 on a frame whose clipping is one region deeper than its gate's
/// end develops as the fill off, to the bit, with no fields and no
/// range shift, where hl-1 fills it; hl-2's list names its version, so
/// nothing kept of one version serves the other.
#[test]
fn hl_2_leaves_a_region_past_its_gate_as_off() {
    // White 500 on the scene's ramp: all but its left edge clips, one
    // region far deeper than the gate's 224 px.
    let input = Input::Raw(clipped_with(640, 600, 500.0));
    let second = |mut e: Edit| {
        e.highlight_fill = Some("hl-2".into());
        e
    };
    let off = walk_cpu(&plain(), &Purpose::Viewport, &input, 0, &mut Kept::new());
    let two = walk_cpu(
        &second(plain()),
        &Purpose::Viewport,
        &input,
        0,
        &mut Kept::new(),
    );
    let one = walk_cpu(
        &on(plain()),
        &Purpose::Viewport,
        &input,
        0,
        &mut Kept::new(),
    );
    assert!(two.filled.is_none(), "no fields past the gate");
    assert!(two.meta.range_shift.is_none());
    assert!(picture(&two).data == picture(&off).data);
    assert_eq!(two.taps.guide, off.taps.guide);
    assert!(
        one.filled.is_some_and(|f| f.changed > 10_000),
        "hl-1 fills it"
    );
    // Each list through its fill: the two differ at the fill alone, and
    // one's fields never stand in for the other's.
    let through = |e: &Edit| {
        let mut steps = stages::stages(e, &Purpose::Viewport, &input.facts(0), None).steps;
        let at = steps
            .iter()
            .position(|s| matches!(s, Step::Op(Stage::HighlightFill(_))))
            .unwrap();
        steps.truncate(at + 1);
        steps
    };
    let (l1, l2) = (through(&on(plain())), through(&second(plain())));
    assert_eq!(l2.last(), Some(&Step::Op(Stage::HighlightFill("hl-2"))));
    assert_eq!(l1[..l1.len() - 1], l2[..l2.len() - 1]);
    assert!(stages::stands_in_for(&l1, &l1));
    assert!(!stages::stands_in_for(&l1, &l2));
}

/// The reduced develop and the models' picture fill their binned base
/// on the full mosaic's cells pooled to it (a binned pixel is one Bayer
/// cell), look-scaled by the bin: the base is core's fill at f = 2 run
/// by hand on the binned picture, to the bit, and moves only binned
/// pixels whose cell clipped. The fill is no skip, so the list's skips
/// and recipe are the ones it has with the fill off.
#[test]
fn the_reduced_develops_fill_their_binned_base() {
    // At white 900, where the fill's fields depend on the look scale; at
    // 2000 this frame's regions are too shallow for the depth tie and
    // the hand-over's width to move them, and f = 1 gives the same.
    let raw = clipped_with(4096, 256, 900.0);
    let input = Input::Raw(raw.clone());
    for purpose in [Purpose::Reduced, Purpose::ModelsSee] {
        let mut off_kept = Kept::new();
        walk_cpu(&plain(), &purpose, &input, 0, &mut off_kept);
        let mut on_kept = Kept::new();
        let walked = walk_cpu(&on(plain()), &purpose, &input, 0, &mut on_kept);
        assert_eq!(walked.meta.bin, 2, "{purpose:?}");
        let filled = walked.filled.clone().expect("the fill ran");
        assert_eq!(filled.look_scale, 2.0);
        let steps = stages::stages(&on(plain()), &purpose, &input.facts(0), None);
        let off_steps = stages::stages(&plain(), &purpose, &input.facts(0), None);
        assert_eq!(steps.skipped, off_steps.skipped);
        assert_eq!(steps.reduced_recipe(), off_steps.reduced_recipe());

        // By hand: the prepare, the bin, the fill at f = 2 on the
        // binned codes, the move into the working space.
        let settings = by_hand_settings(&steps);
        let prepared = prepare_with(&raw, &settings, false, None).unwrap();
        let codes: ClipCodes = prepared.clip_codes.clone().unwrap();
        let binned = BinnedCodes::new(&codes, 2).unwrap();
        let pattern = prepared.pattern.clone().unwrap();
        let (mut rgb, bw, bh) = core_develop::reduced::bin_mosaic(
            &prepared.samples,
            prepared.width,
            prepared.height,
            &pattern,
            2,
        );
        let frame_codes = FrameCodes::new(&codes, 2).unwrap();
        assert_eq!(frame_codes.look_scale(), 2.0);
        let version = greycard_ai::highlight::version("hl-1").unwrap();
        let at = |f: f64| {
            camera_fields(
                &rgb,
                bw,
                bh,
                &Camera::of(&prepared),
                Some(&frame_codes),
                frame_codes.clips(),
                version.weights(),
                &version.look,
                f,
            )
            .unwrap()
        };
        let (fill, unscaled) = (at(2.0), at(1.0));
        // The scene's fields depend on the look scale, so a fill at
        // f = 1 is not this one.
        assert!(fill.fields() != unscaled.fields());
        fill.apply(&mut rgb, &frame_codes);
        let binned_prepared = Prepared {
            samples: Vec::new(),
            width: bw,
            height: bh,
            pattern: None,
            ..prepared
        };
        let want = core_develop::finish(binned_prepared, rgb, None, None, &settings).unwrap();
        let (off, on) = (base(&off_kept), base(&on_kept));
        assert!(on.data == want.image.data, "the fill at f = 2 by hand");
        assert_eq!((on.width, on.height), binned.size());
        let moved = moved(&off, &on);
        assert_eq!(moved.len(), filled.changed);
        assert!(!moved.is_empty());
        assert!(moved.iter().all(|&(x, y)| binned.code(x, y) > 0));
    }
}

/// A frame that takes no fill, or a version this build lacks, walks
/// with the fill on as with it off, to the bit, and says why; the edit
/// keeps its field.
#[test]
fn a_frame_that_takes_no_fill_walks_as_off() {
    let clipped = clipped_frame_of(192, 120);
    let xtrans = RawFrame {
        layout: SensorLayout::Cfa(CfaPattern::xtrans()),
        ..(*clipped).clone()
    };
    let monochrome = RawFrame {
        layout: SensorLayout::Monochrome,
        samples: Samples::U16(
            (0..192 * 120)
                .map(|i| 300 + (i % 192) as u16 * 20)
                .collect(),
        ),
        as_shot_coefficients: Some([1.0; 3]),
        calibrations: Vec::new(),
        ..(*clipped).clone()
    };
    // A linear DNG's: three camera-RGB samples a pixel, the mosaic's
    // values taken for each.
    let Samples::U16(mosaic) = &clipped.samples else {
        unreachable!("the test frame is 16-bit")
    };
    let linear = RawFrame {
        layout: SensorLayout::Linear,
        channels: 3,
        samples: Samples::U16(mosaic.iter().flat_map(|&v| [v; 3]).collect()),
        levels: greycard_core::raw::Levels {
            black: LevelPattern::uniform(64.0, 3),
            white: LevelPattern::uniform(2000.0, 3),
            black_deltas: None,
            saturation: None,
        },
        ..(*clipped).clone()
    };
    let mut merged = (*clipped).clone();
    merged.dng.merged = Some(greycard_core::merge::Tag {
        kind: "hdr".into(),
        ..Default::default()
    });
    let cases = [
        (
            xtrans,
            "on",
            "the highlight fill has no X-Trans version yet",
        ),
        (
            monochrome,
            "on",
            "the highlight fill has no monochrome version",
        ),
        (
            linear,
            "on",
            "the highlight fill needs a mosaic, and this file has none",
        ),
        (merged, "on", "the highlight fill does not run on a merge"),
        ((*clipped).clone(), "hl-9", NO_SUCH_FILL),
    ];
    for (frame, name, why) in cases {
        let input = Input::Raw(Arc::new(frame));
        let mut edit = plain();
        edit.highlight_fill = Some(if name == "on" { "hl-1" } else { name }.into());
        let written = edit.to_json();
        let steps = stages::stages(&edit, &Purpose::Viewport, &input.facts(0), None);
        let off_steps = stages::stages(&plain(), &Purpose::Viewport, &input.facts(0), None);
        assert_eq!(steps.steps, off_steps.steps, "{why}");
        assert!(
            steps
                .skipped
                .contains(&(HIGHLIGHT_FILL, Skip::Unavailable(why)))
        );
        let on = walk_cpu(&edit, &Purpose::Viewport, &input, 0, &mut Kept::new());
        let off = walk_cpu(&plain(), &Purpose::Viewport, &input, 0, &mut Kept::new());
        assert!(on.filled.is_none());
        assert!(on.meta.range_shift.is_none());
        assert!(picture(&on).data == picture(&off).data, "{why}");
        // The edit keeps its field through the skipped develop: written
        // as it was, naming the version, and read back the same.
        let after = edit.to_json();
        assert_eq!(after, written, "{why}");
        let wanted = if name == "on" { "hl-1" } else { name };
        let read = Edit::from_json(&after).unwrap();
        assert_eq!(read.highlight_fill.as_deref(), Some(wanted));
        // And on a frame that takes it, the same edit fills.
        let bayer = Input::Raw(clipped.clone());
        let filled = walk_cpu(&read, &Purpose::Viewport, &bayer, 0, &mut Kept::new());
        assert_eq!(filled.filled.is_some(), name == "on", "{why}");
    }
}

/// The fill lets go of the prepared mosaic's samples, which nothing
/// after the demosaic reads, whether or not it fills, and of the cells
/// once read.
#[test]
fn the_fill_lets_go_of_the_mosaic() {
    let raw = clipped_frame_of(160, 120);
    let input = Input::Raw(raw.clone());
    let steps = stages::stages(&on(plain()), &Purpose::Viewport, &input.facts(0), None);
    let settings = by_hand_settings(&steps);
    for name in ["hl-1", "hl-9"] {
        let mut prepared = prepare_with(&raw, &settings, false, None).unwrap();
        let (mut rgb, _, _) = demosaic_prepared(&prepared, &settings).unwrap();
        assert!(!prepared.samples.is_empty());
        let filled = crate::highlight_fill::run(name, &mut prepared, &mut rgb, None, 1, &|| false)
            .expect("never asked to give way");
        assert_eq!(filled.is_some(), name == "hl-1");
        assert!(prepared.samples.is_empty(), "{name}");
        assert_eq!(prepared.samples.capacity(), 0, "{name}");
    }
    let mut prepared = prepare_with(&raw, &settings, false, None).unwrap();
    let (mut rgb, _, _) = demosaic_prepared(&prepared, &settings).unwrap();
    crate::highlight_fill::run("hl-1", &mut prepared, &mut rgb, None, 1, &|| false)
        .unwrap()
        .unwrap();
    assert!(prepared.clip_codes.is_none(), "the cells let go");
}

/// The guide's range on, less off's, in stops, for one list each.
fn range_drift(off: &Steps, on: &Steps, input: &Input) -> ([f32; 2], Walked) {
    let go = |s: &Steps| {
        let plan = plan(s, &Purpose::Viewport, false);
        walk(
            s,
            &plan,
            input,
            &mut Kept::new(),
            &mut NoModels,
            &mut None,
            &NoHold,
        )
        .unwrap()
    };
    let (off, on) = (go(off), go(on));
    let (a, b) = (
        off.taps.guide.unwrap().range,
        on.taps.guide.clone().unwrap().range,
    );
    ([b[0] - a[0], b[1] - a[1]], on)
}

/// [`clipped_frame_of`]'s scene 640 by 426 with its white at `white`:
/// at 300 nearly all of it clips and the fill lifts the frame's top by
/// two stops; at 900, by one; at 2000, by a few thousandths.
fn clipped_at(white: f32) -> Arc<RawFrame> {
    clipped_with(640, 426, white)
}

/// The whites the range's drift is pinned at.
const WHITES: [f32; 3] = [300.0, 900.0, 2000.0];

/// With a DNG's warp between the fill and the guide's tap, the range
/// is off's to 0.05 stops at the median and the top. Not zero: the
/// shift is measured on the unwarped camera picture, so the warp moves
/// the filled light between the guide's cells after it was measured,
/// and the resample blends it with its neighbors. Measured: up to
/// +0.034 at the top (white 900, the strong warp).
#[test]
fn the_range_under_a_files_warp_drifts_within_its_bound() {
    let mut drifts = Vec::new();
    for (strength, per_plane, white) in [(0.03, false), (0.12, false), (0.08, true)]
        .into_iter()
        .flat_map(|(s, p)| WHITES.map(|w| (s, p, w)))
    {
        let raw = clipped_at(white);
        let mut frame = (*raw).clone();
        let mut p = greycard_core::opcodes::WarpPlane::identity();
        (p.radial[0], p.radial[2]) = (1.0 - strength, strength * 2.0);
        let planes = if per_plane {
            let (mut r, mut b) = (p.clone(), p.clone());
            r.radial[0] *= 1.003;
            b.radial[0] *= 0.997;
            vec![r, p, b]
        } else {
            vec![p]
        };
        frame.dng.warps = vec![greycard_core::opcodes::Warp {
            planes,
            center_x: 0.48,
            center_y: 0.53,
            reciprocal: false,
        }];
        let input = Input::Raw(Arc::new(frame));
        let steps = |e: &Edit| stages::stages(e, &Purpose::Viewport, &input.facts(0), None);
        let (d, walked) = range_drift(&steps(&plain()), &steps(&on(plain())), &input);
        assert!(walked.meta.range_shift.is_some());
        eprintln!("white {white}, warp {strength} per plane {per_plane}: drift {d:?}");
        drifts.push(d);
    }
    for d in drifts {
        assert!(d[0].abs() < 0.05 && d[1].abs() < 0.05, "{d:?}");
    }
}

/// With a lens correction between the fill and the tap, the range
/// drifts: the shift is measured before the correction, which then
/// brightens the frame's edges by its vignetting (changing which cells
/// are the top and the median, and how much the fill lifted them) and
/// moves the filled light between cells by its distortion. Accepted
/// for now (§719); pinned so a regression shows. Measured: vignetting
/// alone up to 0.076 (white 300, the median), bound 0.1; with
/// distortion k1 -0.08 and vignetting -0.6 the top -0.187 (white 900)
/// and the median -0.133 (white 300), bound 0.25.
#[test]
fn the_range_under_a_lens_correction_drifts_within_its_bound() {
    for white in WHITES {
        lens_drift(white);
    }
}

fn lens_drift(white: f32) {
    use greycard_core::develop::lens::{Distortion, LensCorrection, Vignetting};
    let input = Input::Raw(clipped_at(white));
    let with_lens = |mut s: Steps, c: &LensCorrection| {
        let at = s
            .steps
            .iter()
            .position(|x| *x == Step::Keep(Keep::Base))
            .unwrap();
        s.steps.insert(at, Step::Op(Stage::Lens(vec![*c])));
        s
    };
    let steps = |e: &Edit| stages::stages(e, &Purpose::Viewport, &input.facts(0), None);
    for (what, c, bound) in [
        (
            "vignetting",
            LensCorrection {
                vignetting: Some(Vignetting {
                    k: [-0.4, 0.0, 0.0],
                }),
                ..Default::default()
            },
            0.1,
        ),
        (
            "distortion and vignetting",
            LensCorrection {
                distortion: Some(Distortion::Poly3 { k1: -0.08 }),
                vignetting: Some(Vignetting {
                    k: [-0.6, 0.1, 0.0],
                }),
                ..Default::default()
            },
            0.25,
        ),
    ] {
        let (d, walked) = range_drift(
            &with_lens(steps(&plain()), &c),
            &with_lens(steps(&on(plain())), &c),
            &input,
        );
        assert!(walked.meta.range_shift.is_some());
        eprintln!("white {white}, lens {what}: drift {d:?}");
        assert!(
            d[0].abs() < bound && d[1].abs() < bound,
            "white {white}, {what}: {d:?}"
        );
    }
}

/// hl-2 on a region on its gate's shoulder: the walk's base is core's
/// hl-2 fill run by hand on the same picture, to the bit, the viewport,
/// prefetch, and export bases agree to the bit, and the region takes a
/// share of hl-1's fill, neither all nor none of it (its depth0 lies
/// between the gate's ends, read off the fill's own input).
#[test]
fn hl_2_on_its_shoulder_agrees_with_core_and_across_purposes() {
    use greycard_core::develop::highlight_fill::analysis;
    use greycard_core::develop::highlight_fill::half::Plain;

    // White 1640: the ramp clips from about 220 px short of the right
    // edge, over the whole height, a region of depth0 about 196 (the
    // frame's edges are no boundary), on hl-2's shoulder.
    let raw = clipped_with(480, 160, 1640.0);
    let input = Input::Raw(raw.clone());
    let second = |mut e: Edit| {
        e.highlight_fill = Some("hl-2".into());
        e
    };
    let base_of = |e: &Edit, purpose: &Purpose| {
        let mut kept = Kept::new();
        walk_cpu(e, purpose, &input, 0, &mut kept);
        base(&kept)
    };
    let two = base_of(&second(plain()), &Purpose::Viewport);
    let one = base_of(&on(plain()), &Purpose::Viewport);
    let off = base_of(&plain(), &Purpose::Viewport);
    assert!(two.data == base_of(&second(plain()), &Purpose::Prefetch).data);
    assert!(two.data == base_of(&second(plain()), &Purpose::Export(Box::default())).data);

    // By hand: core's fill with hl-2's look on the walk's picture.
    let steps = stages::stages(&second(plain()), &Purpose::Viewport, &input.facts(0), None);
    let settings = by_hand_settings(&steps);
    let prepared = prepare_with(&raw, &settings, false, None).unwrap();
    let codes = prepared.clip_codes.clone().expect("the cells read");
    let (w, h) = (prepared.width, prepared.height);
    let (rgb, dual, denoised) = demosaic_prepared(&prepared, &settings).unwrap();
    let version = greycard_ai::highlight::version("hl-2").unwrap();
    let frame_codes = FrameCodes::new(&codes, 1).unwrap();
    let camera = Camera::of(&prepared);
    let reach = version.look.reach.expect("hl-2 has a gate");
    let x: Vec<f32> = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|&p| camera.working(p).map(|v| v / camera.scale()))
        .collect();
    let a = analysis::analyze_of(
        &Plain::new(&x, w, h),
        &frame_codes,
        frame_codes.clips(),
        version.look.harmonic_bright as f32,
    );
    let deepest = a.regions.iter().fold(0.0f32, |m, r| m.max(r.depth0));
    eprintln!("the deepest region's depth0: {deepest} px");
    assert!(
        (deepest as f64) > reach.gate[0] && (deepest as f64) < reach.gate[1],
        "on the shoulder: {deepest}"
    );
    let fill = camera_fields(
        &rgb,
        w,
        h,
        &camera,
        Some(&frame_codes),
        frame_codes.clips(),
        version.weights(),
        &version.look,
        1.0,
    )
    .expect("fields");
    let mut camera_rgb = rgb.clone();
    fill.apply(&mut camera_rgb, &frame_codes);
    let by_hand = core_develop::finish(prepared, camera_rgb, dual, denoised, &settings).unwrap();
    assert!(by_hand.image.data == two.data, "the walk is core's hl-2");

    // A share of hl-1's fill: some pixels hl-1 moves, hl-2 moves less.
    let (moved_two, moved_one) = (moved(&off, &two), moved(&off, &one));
    assert!(!moved_two.is_empty() && moved_two.len() <= moved_one.len());
    assert!(two.data != one.data, "less than hl-1's whole fill");
}
