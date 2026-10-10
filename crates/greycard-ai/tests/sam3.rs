//! SAM 3 against the real model: set `GREYCARD_MODELS` to a store root
//! that holds `sam3-fp16-top16-1` and run with `--ignored`. The
//! pictures come from outside the repo, by the variables each test
//! names; none enters it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use greycard_ai::sam3::{self, Choice, Encodings, Picture, Rect, Route};
use greycard_ai::{Presets, Provider, Rgb8, Sam3, Store};

fn store() -> Option<Store> {
    std::env::var_os("GREYCARD_MODELS").map(Store::at)
}

fn open(path: &Path) -> Rgb8 {
    let rgb = image::open(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .to_rgb8();
    Rgb8::new(rgb.width() as usize, rgb.height() as usize, rgb.into_raw())
}

fn crop(image: &Rgb8, r: Rect) -> Rgb8 {
    let mut data = Vec::with_capacity(r.width() * r.height() * 3);
    for y in r.y0..r.y1 {
        data.extend(&image.data[(y * image.width + r.x0) * 3..(y * image.width + r.x1) * 3]);
    }
    Rgb8::new(r.width(), r.height(), data)
}

/// The picture at the editor's preview size, 2048 on the long side
/// (or as it is, if smaller).
fn preview_of(full: &Rgb8) -> Rgb8 {
    let long = full.width.max(full.height);
    if long <= 2048 {
        return full.clone();
    }
    let s = 2048.0 / long as f64;
    sam3::resample(
        full,
        (full.width as f64 * s).round() as usize,
        (full.height as f64 * s).round() as usize,
    )
}

fn providers() -> Vec<Provider> {
    let mut out = vec![Provider::Cpu];
    if Provider::available().contains(&Provider::WebGpu) {
        out.push(Provider::WebGpu);
    } else {
        println!("no WebGPU here; the CPU is the whole test");
    }
    out
}

/// Decodes three presets on a picture and compares them with what the
/// Python side's ONNX Runtime gave (`tools/ai/sam3_trial/reference.py`,
/// its JSON named by `GREYCARD_SAM3_REFERENCE`): the presence, all
/// sixteen scores, the count over the cut, the best box and its mask's
/// mean, on the CPU to 1e-3 and on WebGPU to 2e-2 (see below), the
/// count over the cut exactly. Also checks the squash is Pillow's to the byte,
/// and that the person's signature is the same on both providers.
#[test]
#[ignore]
fn the_decoder_answers_as_the_python_reference() {
    let Some(store) = store() else { return };
    let path = std::env::var_os("GREYCARD_SAM3_REFERENCE")
        .map(PathBuf::from)
        .expect("GREYCARD_SAM3_REFERENCE: the JSON tools/ai/sam3_trial/reference.py writes");
    let reference: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let picture = open(Path::new(reference["picture"].as_str().unwrap()));
    let squashed = open(Path::new(reference["squashed"].as_str().unwrap()));
    assert_eq!(
        sam3::resample(&picture, sam3::SIDE, sam3::SIDE),
        squashed,
        "the squash is not Pillow's"
    );

    let presets = Presets::of(&store).expect("the phrase table");
    let phrases = reference["phrases"].as_object().unwrap();
    let mut signatures = Vec::new();
    for provider in providers() {
        let t = Instant::now();
        let mut sam = Sam3::load(&store, &[provider]).expect("load");
        assert_eq!(sam.providers(), (provider, provider));
        println!(
            "{} loaded in {:.1}s",
            provider.name(),
            t.elapsed().as_secs_f64()
        );
        let t = Instant::now();
        let enc = sam.encode(&picture).expect("encode");
        println!("  encode {:.2}s", t.elapsed().as_secs_f64());
        // On the CPU the same arithmetic as Python's ONNX Runtime, to
        // 2.7e-4 measured. WebGPU runs the fp16 encoder's arithmetic
        // in half precision on the card: a score moved by up to 8.7e-3
        // and the presence by 1.1e-3 on the reference picture, all of
        // it the encoder's (the CPU decoder fed the card's encoding
        // gives the card's numbers), and never across the cut.
        let tolerance = if provider == Provider::Cpu {
            1e-3
        } else {
            2e-2
        };
        let mut worst = 0f32;
        for (text, want) in phrases {
            let t = Instant::now();
            let found = sam
                .decode(&enc, presets.get(text).unwrap())
                .expect("decode");
            let dt = t.elapsed().as_secs_f64();
            let presence = (found.presence - want["presence"].as_f64().unwrap() as f32).abs();
            let scores: Vec<f32> = want["scores"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect();
            assert_eq!(found.instances.len(), scores.len());
            let mut score = 0f32;
            for (i, s) in found.instances.iter().zip(&scores) {
                score = score.max((i.score - s).abs());
            }
            let best = &found.instances[0];
            let mut bbox = 0f32;
            for (a, b) in best.bbox.iter().zip(want["best_box"].as_array().unwrap()) {
                bbox = bbox.max((a - b.as_f64().unwrap() as f32).abs());
            }
            let mean = best.mask.data.iter().sum::<f32>() / best.mask.data.len() as f32;
            let mean = (mean - want["best_mask_mean"].as_f64().unwrap() as f32).abs();
            let diff = presence.max(score).max(bbox).max(mean);
            let over = found.over(sam3::CUT).count();
            println!(
                "  {text}: presence {:.4}, best {:.4}, {over} over the cut, decode {dt:.2}s; \
                 differences: presence {presence:.1e}, scores {score:.1e}, box {bbox:.1e}, \
                 mask mean {mean:.1e}",
                found.presence, best.score
            );
            assert_eq!(over as u64, want["found"].as_u64().unwrap(), "{text}");
            worst = worst.max(diff);
        }
        println!(
            "  {}: largest difference from Python {worst:.2e}",
            provider.name()
        );
        assert!(worst < tolerance, "{}: {worst}", provider.name());

        let preview = preview_of(&picture);
        let luma = preview.luma();
        let mut region = |r: Rect| crop(&picture, r);
        let mut encodings = Encodings::new(4);
        let mut pic = Picture {
            preview: &preview,
            luma: &luma,
            size: (picture.width, picture.height),
            key: 1,
            region: &mut region,
            encodings: &mut encodings,
            matte: None,
        };
        let people = sam3::people(&mut sam, &presets, &mut pic).expect("people");
        assert!(!people.is_empty(), "nobody on the reference picture");
        signatures.push(people[0].signature.clone());
    }
    if let [cpu, gpu] = &signatures[..] {
        let d = sam3::distance(cpu, gpu).expect("comparable");
        let most = cpu
            .values()
            .iter()
            .zip(gpu.values())
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        println!(
            "the person's signature, CPU against WebGPU: distance {d:.3}, \
             most apart in a value {most:.4}"
        );
        assert!(sam3::same_render(cpu, gpu), "{most}");
    }
}

/// The Eye route on the picture named by `GREYCARD_SAM3_PICTURE` (one
/// face, both eyes open and in view; full resolution or a preview),
/// on every provider: two irises, each a crop of the picture with a
/// mask in it. Prints the Mouth route's lips and teeth too.
#[test]
#[ignore]
fn the_eye_route_finds_two_irises() {
    let Some(store) = store() else { return };
    let path = std::env::var_os("GREYCARD_SAM3_PICTURE")
        .map(PathBuf::from)
        .expect("GREYCARD_SAM3_PICTURE: a picture of one face, both eyes in view");
    let full = open(&path);
    let preview = preview_of(&full);
    let luma = preview.luma();
    let presets = Presets::of(&store).expect("the phrase table");
    for provider in providers() {
        let mut sam = Sam3::load(&store, &[provider]).expect("load");
        let mut region = |r: Rect| crop(&full, r);
        let mut encodings = Encodings::new(8);
        let mut pic = Picture {
            preview: &preview,
            luma: &luma,
            size: (full.width, full.height),
            key: 1,
            region: &mut region,
            encodings: &mut encodings,
            matte: None,
        };
        let t = Instant::now();
        let irises = sam3::find(
            &mut sam,
            &presets,
            &mut pic,
            Route::Eye,
            "iris of the eye",
            None,
        )
        .expect("the eye route");
        let dt = t.elapsed().as_secs_f64();
        let mut sizes = Vec::new();
        for p in &irises {
            let on = p.mask.data.iter().filter(|&&v| v > 0.5).count();
            let share = on as f32 / p.mask.data.len() as f32;
            println!(
                "  iris in {:?}: {on} px over a half, {:.1}% of the crop",
                p.rect,
                share * 100.0
            );
            sizes.push((on, share));
        }
        println!("{}: {} irises in {dt:.2}s", provider.name(), irises.len());
        assert_eq!(irises.len(), 2);
        // An iris is a few dozen pixels at least, and never most of a
        // crop two and a half eyes wide (a smiling eye looking aside on
        // a 2048 preview: 149 px, 0.7%).
        for (on, share) in sizes {
            assert!(on >= 40 && share < 0.25, "{on} px, {share}");
        }
        assert_ne!(irises[0].rect, irises[1].rect);
        for phrase in ["lips", "teeth"] {
            let t = Instant::now();
            let pieces = sam3::find(&mut sam, &presets, &mut pic, Route::Mouth, phrase, None)
                .expect("the mouth route");
            println!(
                "  {phrase}: {} pieces in {:.2}s",
                pieces.len(),
                t.elapsed().as_secs_f64()
            );
        }
    }
}

/// The frames the match bound was set on, in the directory named by
/// `GREYCARD_SAM3_FRAMES` (2048 previews): 5M0A4169 is five people,
/// 4Z4A3846 and 5M0A0504 a couple each, the rest one person; the
/// pairs in `SAME` are one woman in two frames, everyone else is
/// someone different.
const FRAMES: &[&str] = &[
    "066A3439", "4Z4A3846", "5F5A0092", "5M0A0067", "5M0A0504", "5M0A2279", "5M0A3021", "5M0A4154",
    "5M0A4160", "5M0A4169", "DSCF0835", "DSCF0848", "P1000247",
];
const SAME: &[(&str, &str)] = &[("DSCF0835", "DSCF0848"), ("5M0A4154", "5M0A4160")];

/// People on the frames, every person's signature asked of every other
/// frame: never a wrong person decided; each woman of `SAME` found
/// again on her other frame (a picture of one person); every picture
/// of two or more asked about; each of the five on the group frame
/// themself on their own frame. `BOUND` was tuned on these same
/// frames, so passing here shows the rule holds where it was set, not
/// that it generalizes: on four other shoots colors did not tell
/// people in groups apart at all, which is why a group is always asked
/// about. Prints the distances and the tally of every outcome.
#[test]
#[ignore]
fn people_are_told_apart_on_the_frames() {
    let Some(store) = store() else { return };
    let dir = std::env::var_os("GREYCARD_SAM3_FRAMES")
        .map(PathBuf::from)
        .expect("GREYCARD_SAM3_FRAMES: the directory of the thirteen 2048 previews");
    let presets = Presets::of(&store).expect("the phrase table");
    let mut sam = Sam3::load(&store, &Provider::available()).expect("load");
    let mut all: Vec<(&str, Vec<sam3::Person>)> = Vec::new();
    for (key, stem) in FRAMES.iter().enumerate() {
        let preview = open(&dir.join(format!("{stem}.jpg")));
        let luma = preview.luma();
        let mut region = |r: Rect| crop(&preview, r);
        let mut encodings = Encodings::new(0);
        let mut pic = Picture {
            preview: &preview,
            luma: &luma,
            size: (preview.width, preview.height),
            key: key as u64,
            region: &mut region,
            encodings: &mut encodings,
            matte: None,
        };
        let people = sam3::people(&mut sam, &presets, &mut pic).expect("people");
        for (i, p) in people.iter().enumerate() {
            let b = p.signature.bands().unwrap();
            let l = |k: usize| {
                if b[k].is_there() {
                    format!("{:.0}", b[k].mean[0])
                } else {
                    "-".into()
                }
            };
            println!(
                "{stem} #{i}: face {:.2} at ({:.2}, {:.2}), size {:.3}, L {}/{}/{}",
                p.score,
                p.at[0],
                p.at[1],
                p.signature.size().unwrap(),
                l(0),
                l(1),
                l(2),
            );
        }
        all.push((stem, people));
    }
    let on = |stem: &str| -> Vec<(sam3::Signature, [f32; 2])> {
        all.iter()
            .find(|(s, _)| *s == stem)
            .unwrap()
            .1
            .iter()
            .map(|p| (p.signature.clone(), p.at))
            .collect()
    };
    let same_pair = |a: &str, b: &str| {
        SAME.iter()
            .any(|&(x, y)| (x, y) == (a, b) || (y, x) == (a, b))
    };

    let mut others = Vec::new();
    let mut incomparable = 0;
    for (i, (a, pa)) in all.iter().enumerate() {
        for (b, pb) in &all[i + 1..] {
            if same_pair(a, b) {
                continue;
            }
            for (x, p) in pa.iter().enumerate() {
                for (y, q) in pb.iter().enumerate() {
                    match sam3::distance(&p.signature, &q.signature) {
                        Some(d) => others.push((d, *a, x, *b, y)),
                        None => incomparable += 1,
                    }
                }
            }
        }
    }
    others.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!(
        "{} pairs of different people on different frames, {incomparable} not comparable; \
         the nearest:",
        others.len() + incomparable
    );
    for (d, a, x, b, y) in others.iter().take(5) {
        println!("  {d:.2}  {a} #{x} and {b} #{y}");
    }
    // No two people's signatures, on one picture or two, the same
    // woman's two frames among them, are the same render.
    let everyone: Vec<(&str, usize, &sam3::Signature)> = all
        .iter()
        .flat_map(|(stem, people)| {
            people
                .iter()
                .enumerate()
                .map(move |(i, p)| (*stem, i, &p.signature))
        })
        .collect();
    let mut nearest_values = (f32::INFINITY, "", 0, "", 0);
    for (k, &(a, x, p)) in everyone.iter().enumerate() {
        for &(b, y, q) in &everyone[k + 1..] {
            let most = p
                .values()
                .iter()
                .zip(q.values())
                .map(|(u, v)| (u - v).abs())
                .fold(0f32, f32::max);
            if most < nearest_values.0 {
                nearest_values = (most, a, x, b, y);
            }
            assert!(!sam3::same_render(p, q), "{a} #{x} and {b} #{y}");
        }
    }
    let (most, a, x, b, y) = nearest_values;
    println!("two people nearest in every value: {a} #{x} and {b} #{y}, {most:.2} apart");

    for &(a, b) in SAME {
        let (pa, pb) = (on(a), on(b));
        assert_eq!((pa.len(), pb.len()), (1, 1), "{a}, {b}: one woman each");
        println!(
            "the same woman, {a} to {b}: {:.2}",
            sam3::distance(&pa[0].0, &pb[0].0).expect("comparable")
        );
    }

    // Every person asked of every other frame.
    let (mut right, mut wrong, mut missed, mut nobody) = (0, 0, 0, 0);
    let (mut asked, mut guess_right, mut guess_wrong, mut no_guess) = (0, 0, 0, 0);
    for (a, pa) in &all {
        for her in pa {
            for b in FRAMES.iter().filter(|&&b| b != *a) {
                let there = on(b);
                // Who she is there, if she is: the one woman on her
                // other frame of a `SAME` pair.
                let truth = same_pair(a, b).then_some(0usize);
                match sam3::choose(&her.signature, her.at, &there) {
                    Choice::Person(i, _) if Some(i) == truth => right += 1,
                    Choice::Person(i, d) => {
                        wrong += 1;
                        println!("WRONG: {a} taken for {b} #{i} at {d:.2}");
                    }
                    Choice::Nobody => {
                        if truth.is_some() {
                            missed += 1;
                            println!("missed: {a} on {b}");
                        } else {
                            nobody += 1;
                        }
                    }
                    Choice::Ask { guess } => {
                        assert!(there.len() >= 2 || guess.is_none(), "{a} on {b}");
                        asked += 1;
                        match guess {
                            None => no_guess += 1,
                            Some(g) if Some(g) == truth => guess_right += 1,
                            Some(g) => {
                                guess_wrong += 1;
                                println!("guess: {a} on {b}, #{g} (not her)");
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "decided: {right} right, {wrong} wrong, {missed} missed, {nobody} nobody; \
         asked: {asked} ({guess_right} with her as the guess, {guess_wrong} with someone \
         else, {no_guess} with none)"
    );
    assert_eq!(wrong, 0, "a wrong person decided");
    assert_eq!(missed, 0, "a woman not found on her other frame");
    assert_eq!(right, 2 * SAME.len());

    // On the group frame, each of the five is themself.
    let group = on("5M0A4169");
    assert_eq!(group.len(), 5, "the group frame");
    for (i, p) in group.iter().enumerate() {
        let c = sam3::choose(&p.0, p.1, &group);
        assert!(matches!(c, Choice::Person(j, _) if j == i), "#{i}: {c:?}");
    }
}

/// The soft band of a mask `w` wide: its pixels between 0.1 and 0.9 for
/// each pixel of its edge (over a half with a neighbor under it). The
/// wider, the fuzzier the edge reads.
fn band(m: &[f32], w: usize) -> f64 {
    let h = m.len() / w;
    let soft = m.iter().filter(|&&v| v > 0.1 && v < 0.9).count();
    let mut edge = 0usize;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if m[i] < 0.5 {
                continue;
            }
            let under = (x > 0 && m[i - 1] < 0.5)
                || (x + 1 < w && m[i + 1] < 0.5)
                || (y > 0 && m[i - w] < 0.5)
                || (y + 1 < h && m[i + w] < 0.5);
            edge += under as usize;
        }
    }
    soft as f64 / edge.max(1) as f64
}

/// Two masks' pixels over a half: their intersection over their union.
fn iou(a: &[f32], b: &[f32]) -> f64 {
    let both = a
        .iter()
        .zip(b)
        .filter(|&(&x, &y)| x > 0.5 && y > 0.5)
        .count();
    let either = a
        .iter()
        .zip(b)
        .filter(|&(&x, &y)| x > 0.5 || y > 0.5)
        .count();
    both as f64 / either.max(1) as f64
}

/// Pixels both masks are over `at` in.
fn shared(a: &[f32], b: &[f32], at: f32) -> usize {
    a.iter().zip(b).filter(|&(&x, &y)| x > at && y > at).count()
}

/// Crops of a frame at 1:1 and twice that: SAM's edge, Subject's edge
/// and Subject's matte side by side, each over the picture in red and
/// alone below it. `spec` is `x,y,w,h` in the preview's pixels.
fn save_crops(out: &Path, stem: &str, spec: &str, preview: &Rgb8, panels: &[greycard_ai::Mask]) {
    let r: Vec<usize> = spec.split(',').map(|v| v.trim().parse().unwrap()).collect();
    let [x0, y0, cw, ch] = [r[0], r[1], r[2], r[3]];
    let pw = preview.width;
    for scale in [1usize, 2] {
        let gap = 4;
        let (ow, oh) = (
            panels.len() * (cw * scale + gap) - gap,
            2 * ch * scale + gap,
        );
        let mut img = vec![255u8; ow * oh * 3];
        for (k, m) in panels.iter().enumerate() {
            for y in 0..ch * scale {
                for x in 0..cw * scale {
                    let (sx, sy) = (x0 + x / scale, y0 + y / scale);
                    let p = &preview.data[(sy * pw + sx) * 3..(sy * pw + sx) * 3 + 3];
                    let v = m.at(sx, sy).clamp(0.0, 1.0);
                    let ox = k * (cw * scale + gap) + x;
                    let top = (y * ow + ox) * 3;
                    let keep = 1.0 - 0.6 * v;
                    img[top] = (p[0] as f32 * keep + 255.0 * 0.6 * v) as u8;
                    img[top + 1] = (p[1] as f32 * keep) as u8;
                    img[top + 2] = (p[2] as f32 * keep) as u8;
                    let low = ((y + ch * scale + gap) * ow + ox) * 3;
                    let g = (v * 255.0).round() as u8;
                    img[low..low + 3].copy_from_slice(&[g, g, g]);
                }
            }
        }
        image::save_buffer(
            out.join(format!("{stem}-crop-{x0}-{y0}-x{scale}.png")),
            &img,
            ow as u32,
            oh as u32,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    }
}

/// Whole person with Subject's edge against SAM's alone, on the frames
/// of `GREYCARD_SAM3_FRAMES`, everyone and each person on a picture of
/// several, drawn into the editor's raster (2048 wide): the soft band
/// per edge pixel ([`band`]) before and after, and Subject's own; All
/// people's IoU with Subject; on a picture of several, the pixels two
/// people share, which Subject's edge must not add to; the seconds
/// Subject and the composing add. With `GREYCARD_EDGE_OUT` set, writes
/// each raster there, and with `GREYCARD_EDGE_CROPS`
/// (`stem:x,y,w,h;...`, the preview's pixels) each crop ([`save_crops`]).
#[test]
#[ignore]
fn whole_person_takes_subjects_edge_on_the_frames() {
    let Some(store) = store() else { return };
    let dir = std::env::var_os("GREYCARD_SAM3_FRAMES")
        .map(PathBuf::from)
        .expect("GREYCARD_SAM3_FRAMES: the directory of the thirteen 2048 previews");
    let out = std::env::var_os("GREYCARD_EDGE_OUT").map(PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out).unwrap();
    }
    let crops = std::env::var("GREYCARD_EDGE_CROPS").unwrap_or_default();
    let presets = Presets::of(&store).expect("the phrase table");
    let providers = Provider::available();
    let mut sam = Sam3::load(&store, &providers).expect("load");
    let t = Instant::now();
    let mut subject = greycard_ai::Subject::load(&store, &providers).expect("Subject");
    println!(
        "Subject ({}) loaded on {} in {:.2}s",
        subject.model().id,
        subject.provider().name(),
        t.elapsed().as_secs_f64()
    );
    let (mut widest_before, mut widest_after) = (0f64, 0f64);
    for (key, stem) in FRAMES.iter().enumerate() {
        let preview = open(&dir.join(format!("{stem}.jpg")));
        let luma = preview.luma();
        let (pw, ph) = (preview.width, preview.height);
        let raster = (2048usize, (2048.0 * ph as f32 / pw as f32).round() as usize);
        let t = Instant::now();
        let matte = subject.mask(&preview).expect("Subject's matte");
        let matte = greycard_ai::refine(&matte, &luma, pw, ph, (pw / 256).max(2), 1e-3);
        let subject_s = t.elapsed().as_secs_f64();
        let whole = Rect {
            x0: 0,
            y0: 0,
            x1: pw,
            y1: ph,
        };
        let drawn = |m: &greycard_ai::Mask| {
            sam3::draw(
                &[sam3::Piece {
                    rect: whole,
                    mask: m.clone(),
                }],
                (pw, ph),
                raster,
            )
        };
        let subj = drawn(&matte);
        let mut region = |r: Rect| crop(&preview, r);
        let mut encodings = Encodings::new(0);
        let mut pic = Picture {
            preview: &preview,
            luma: &luma,
            size: (pw, ph),
            key: key as u64,
            region: &mut region,
            encodings: &mut encodings,
            matte: None,
        };
        let people = sam3::people(&mut sam, &presets, &mut pic).expect("people");
        let mut whos: Vec<Option<usize>> = vec![None];
        if people.len() > 1 {
            whos.extend((0..people.len()).map(Some));
        }
        let mut each = Vec::new();
        for who in whos {
            let person = who.map(|i| &people[i]);
            let mut ask = |pic: &mut Picture<_>| {
                let t = Instant::now();
                let piece = sam3::find(&mut sam, &presets, pic, Route::Whole, sam3::PERSON, person)
                    .expect("Whole person")
                    .remove(0);
                (drawn(&piece.mask), t.elapsed().as_secs_f64())
            };
            pic.matte = None;
            let (before, tb) = ask(&mut pic);
            pic.matte = Some(&matte);
            let (after, ta) = ask(&mut pic);
            let name = who.map_or("all".to_string(), |i| format!("#{i}"));
            let (bb, ba) = (band(&before, raster.0), band(&after, raster.0));
            let share =
                |m: &[f32]| 100.0 * m.iter().filter(|&&v| v > 0.5).count() as f64 / m.len() as f64;
            println!(
                "{stem} {name:>3}: band {bb:5.1} -> {ba:5.1} (Subject {:5.1}); IoU with Subject \
                 {:.3} -> {:.3}; {:.2}% -> {:.2}% of the frame; find {tb:.2}s -> {ta:.2}s, \
                 Subject {subject_s:.2}s",
                band(&subj, raster.0),
                iou(&before, &subj),
                iou(&after, &subj),
                share(&before),
                share(&after),
            );
            widest_before = widest_before.max(bb);
            widest_after = widest_after.max(ba);
            if let Some(out) = &out {
                for (what, m) in [("sam", &before), ("subject-edge", &after)] {
                    let bytes: Vec<u8> = m.iter().map(|v| (v * 255.0).round() as u8).collect();
                    image::save_buffer(
                        out.join(format!("{stem}-{name}-{what}.png")),
                        &bytes,
                        raster.0 as u32,
                        raster.1 as u32,
                        image::ExtendedColorType::L8,
                    )
                    .unwrap();
                }
                if who.is_none() {
                    let at_preview = |m: &[f32]| {
                        greycard_ai::Mask::new(raster.0, raster.1, m.to_vec()).resampled(pw, ph)
                    };
                    let panels = [at_preview(&before), at_preview(&after), matte.clone()];
                    let prefix = format!("{stem}:");
                    for spec in crops.split(';').filter(|s| s.starts_with(&prefix)) {
                        save_crops(out, stem, &spec[prefix.len()..], &preview, &panels);
                    }
                }
            }
            if who.is_some() {
                each.push((name, before, after));
            }
        }
        for i in 0..each.len() {
            for j in i + 1..each.len() {
                let (a, b) = (&each[i], &each[j]);
                println!(
                    "{stem} {} and {}: share {} -> {} pixels over a half, {} -> {} over 0.1",
                    a.0,
                    b.0,
                    shared(&a.1, &b.1, 0.5),
                    shared(&a.2, &b.2, 0.5),
                    shared(&a.1, &b.1, 0.1),
                    shared(&a.2, &b.2, 0.1),
                );
                assert!(
                    shared(&a.2, &b.2, 0.5) <= shared(&a.1, &b.1, 0.5),
                    "{stem}: Subject's edge gave {} and {} pixels in common",
                    a.0,
                    b.0
                );
            }
        }
    }
    println!("widest band: {widest_before:.1} -> {widest_after:.1}");
}
