//! The Sky shape's matte over a folder of real frames, for measuring a
//! change to it: each frame's gate verdict and core, the matte's
//! recall and spill against the labels, the unlabeled pixels inside
//! the found sky and how many of them the matte leaves as holes, the
//! stages' times, and the alpha as a PNG. A measurement, not the
//! product: the numbers it prints are for the notes.
//!
//! Set `GREYCARD_MODELS` to a store holding the Sky model (and SAM 2.1,
//! for SAM's outline as the editor makes it), then
//!
//! ```text
//! SKY_IN=DIR SKY_OUT=DIR cargo run --release -p greycard-ai --example sky_matte
//! ```
//!
//! `SKY_IN` holds each frame's models' preview as `<stem>.png` (the
//! editor's `ai::preview`) and its linear base as `<stem>.lin` (b"GCLN",
//! u32 width, u32 height, then f16 RGB in the working space, of the
//! preview's aspect), as greycard-ui's ignored `dump_the_models_inputs`
//! writes them. The matte is made from the `.lin` at its own size, as
//! the editor makes it from the base, and brought to the preview's
//! size, the raster's. A frame without a `.lin` is passed over.
//!
//! - `SKY_ONLY`: a comma list of stems.
//! - `SKY_NO_SAM`: the prior alone, as the editor runs without SAM: its
//!   own labels are every outline.
//! - `SKY_PROVIDER`: `cpu` (the default) or `webgpu`, for the prior and
//!   SAM both.
//! - `SKY_NONE`: a comma list of stems with no sky; the run fails if
//!   any gets a pixel.
//! - `SKY_COMPARE=A,B`: no model run; the alphas of two earlier runs'
//!   `SKY_OUT`s compared frame by frame (the largest difference, the
//!   mean, the pixels across a half).
//! - `SKY_REGIONS=1`: each region of unlabeled pixels inside the sky
//!   (below) of 20 pixels or more printed with its soft maps, over all
//!   the queries and over the kept ones, against its border's, and the
//!   matte's mean over it.
//!
//! Written to `SKY_OUT`: `<stem>.alpha.png` (the matte, 0 to 255),
//! `<stem>.over.png` (the preview with the matte laid over it in red,
//! for a contact sheet), `<stem>.holes.png` (white where an unlabeled
//! pixel inside the found sky is a hole, grey where it is not), and
//! `table.tsv`, one line a frame, which is also printed.
//!
//! The measures, all at the preview's size:
//!
//! - *core*: the gate's own, the labeled sky over 0.8 on the soft map
//!   over the kept queries, eroded by 1% of the long side, as a share
//!   of the frame;
//! - *recall*: the share of the labeled sky the matte holds over a
//!   half; *spill*: what it holds over a half outside the labels, as a
//!   share of the labels' area (as the landscape's real-frame test
//!   measures its groups);
//! - *partial*: the share of the labeled sky the matte holds between
//!   0.05 and 0.95, where a plateau would show;
//! - *unlabeled inside*: the pixels no kept query claims whose
//!   connected region (four ways) borders only labeled sky and the
//!   frame's edge, as a share of the frame; *holes*: of those, the
//!   share the matte holds under 0.85 inside a region it holds at 0.95
//!   or less, so is surrounded by alpha over 0.95, that either touches
//!   no edge of the frame or never falls under a half (a dip in the
//!   sky running off the frame, not the ground);
//! - *all holes*: every pixel under 0.85 in such a region, of the
//!   frame (a lone tree in the sky counts here, rightly not above).

use std::path::{Path, PathBuf};
use std::time::Instant;

use greycard_ai::sky::{self, Frame, Prior};
use greycard_ai::{Mask, Prompt, Provider, Rgb8, Sam, Sky, Store};
use greycard_core::image::WorkingImage;

fn list(name: &str) -> Option<Vec<String>> {
    std::env::var(name)
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.split(',').map(str::to_string).collect())
}

fn read_lin(path: &Path) -> Option<WorkingImage> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 12 || &bytes[..4] != b"GCLN" {
        return None;
    }
    let w = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let h = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if bytes.len() != 12 + w * h * 6 {
        return None;
    }
    let mut image = WorkingImage::new(w, h);
    for (o, b) in image.data.iter_mut().zip(bytes[12..].as_chunks::<2>().0) {
        *o = half::f16::from_le_bytes(*b).to_f32();
    }
    Some(image)
}

fn picture(path: &Path) -> Rgb8 {
    let rgb = image::open(path).unwrap().to_rgb8();
    Rgb8::new(rgb.width() as usize, rgb.height() as usize, rgb.into_raw())
}

/// The connected regions (four ways) of `set`, each as its pixels.
fn regions(set: &[bool], w: usize, h: usize) -> Vec<Vec<usize>> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    for start in 0..w * h {
        if !set[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let mut region = Vec::new();
        while let Some(i) = stack.pop() {
            region.push(i);
            let (x, y) = (i % w, i / w);
            for j in [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ]
            .into_iter()
            .flatten()
            {
                if set[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        out.push(region);
    }
    out
}

/// Each region's four-way neighbors outside it.
fn borders(region: &[usize], inside: &[bool], w: usize, h: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for &i in region {
        let (x, y) = (i % w, i / w);
        for j in [
            (x > 0).then(|| i - 1),
            (x + 1 < w).then(|| i + 1),
            (y > 0).then(|| i - w),
            (y + 1 < h).then(|| i + w),
        ]
        .into_iter()
        .flatten()
        {
            if !inside[j] {
                out.push(j);
            }
        }
    }
    out
}

/// The unlabeled pixels whose region borders only labeled sky (and
/// the frame's edge).
fn unlabeled_inside(prior: &Prior) -> Vec<bool> {
    let (w, h) = (prior.width, prior.height);
    let unlabeled: Vec<bool> = prior.labels.iter().map(|&l| l == sky::UNLABELED).collect();
    let mut out = vec![false; w * h];
    for region in regions(&unlabeled, w, h) {
        if borders(&region, &unlabeled, w, h)
            .iter()
            .all(|&j| prior.labels[j] == sky::SKY_CLASS)
        {
            for i in region {
                out[i] = true;
            }
        }
    }
    out
}

/// The pixels under 0.85 in a region held at 0.95 or less that
/// touches no edge of the frame or never falls under a half.
fn holes(matte: &Mask) -> Vec<bool> {
    let (w, h) = (matte.width, matte.height);
    let low: Vec<bool> = matte.data.iter().map(|&a| a <= 0.95).collect();
    let mut out = vec![false; w * h];
    for region in regions(&low, w, h) {
        let edge = region.iter().any(|&i| {
            let (x, y) = (i % w, i / w);
            x == 0 || y == 0 || x + 1 == w || y + 1 == h
        });
        let dip = region.iter().all(|&i| matte.data[i] >= 0.5);
        if !edge || dip {
            for i in region {
                out[i] = matte.data[i] < 0.85;
            }
        }
    }
    out
}

#[derive(Default)]
struct Row {
    stem: String,
    verdict: String,
    found: bool,
    seeded: bool,
    labeled: f32,
    core: f32,
    recall: f32,
    spill: f32,
    partial: f32,
    inside: f32,
    inside_px: usize,
    hole_px: usize,
    all_holes: f32,
    area: f32,
    t_model: f64,
    t_labels: f64,
    t_outline: f64,
    t_edge: f64,
    size: (usize, usize),
}

const HEADER: &str = "frame\tverdict\tseeded\tlabeled%\tcore%\tarea%\trecall%\tspill%\tpartial%\t\
                      unlabeled-in%\tunlabeled-in-px\tholes-px\tholes%-of-in\tall-holes%\tmodel-s\t\
                      labels-s\toutline-s\tmatte-s\tlinear";

impl Row {
    fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.3}\t{}\t{}\t{:.1}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{}x{}",
            self.stem,
            self.verdict,
            if self.seeded { "sam" } else { "labels" },
            self.labeled * 100.0,
            self.core * 100.0,
            self.area * 100.0,
            self.recall * 100.0,
            self.spill * 100.0,
            self.partial * 100.0,
            self.inside * 100.0,
            self.inside_px,
            self.hole_px,
            if self.inside_px > 0 {
                self.hole_px as f32 / self.inside_px as f32 * 100.0
            } else {
                0.0
            },
            self.all_holes * 100.0,
            self.t_model,
            self.t_labels,
            self.t_outline,
            self.t_edge,
            self.size.0,
            self.size.1,
        )
    }
}

fn save_gray(path: &Path, w: usize, h: usize, data: Vec<u8>) {
    image::GrayImage::from_raw(w as u32, h as u32, data)
        .expect("a plane of the size")
        .save(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// The preview with the matte laid over it in red.
fn over(picture: &Rgb8, matte: &Mask) -> image::RgbImage {
    let mut out = Vec::with_capacity(picture.data.len());
    for (p, &a) in picture.data.chunks(3).zip(&matte.data) {
        let a = a.clamp(0.0, 1.0) * 0.7;
        let red = [235.0, 60.0, 60.0];
        for c in 0..3 {
            out.push((p[c] as f32 * (1.0 - a) + red[c] * a).round() as u8);
        }
    }
    image::RgbImage::from_raw(picture.width as u32, picture.height as u32, out).unwrap()
}

fn compare(a: &Path, b: &Path) {
    let mut stems: Vec<String> = std::fs::read_dir(a)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_suffix(".alpha.png").map(str::to_string)
        })
        .collect();
    stems.sort();
    println!("frame\tmax/255\tmean/255\tacross-half%");
    for stem in stems {
        let read = |d: &Path| {
            image::open(d.join(format!("{stem}.alpha.png")))
                .ok()
                .map(|i| i.to_luma8())
        };
        let (Some(x), Some(y)) = (read(a), read(b)) else {
            println!("{stem}\tmissing");
            continue;
        };
        let n = x.as_raw().len() as f64;
        let (mut max, mut sum, mut flips) = (0u8, 0u64, 0usize);
        for (p, q) in x.as_raw().iter().zip(y.as_raw()) {
            max = max.max(p.abs_diff(*q));
            sum += p.abs_diff(*q) as u64;
            flips += ((*p > 127) != (*q > 127)) as usize;
        }
        println!(
            "{stem}\t{max}\t{:.4}\t{:.4}",
            sum as f64 / n,
            flips as f64 / n * 100.0
        );
    }
}

fn main() {
    if let Some(pair) = list("SKY_COMPARE") {
        assert_eq!(pair.len(), 2, "SKY_COMPARE wants two folders");
        compare(Path::new(&pair[0]), Path::new(&pair[1]));
        return;
    }
    let store = Store::at(std::env::var_os("GREYCARD_MODELS").expect("GREYCARD_MODELS"));
    let input = PathBuf::from(std::env::var_os("SKY_IN").expect("SKY_IN"));
    let out = PathBuf::from(std::env::var_os("SKY_OUT").expect("SKY_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let only = list("SKY_ONLY");
    let none = list("SKY_NONE").unwrap_or_default();
    let providers = match std::env::var("SKY_PROVIDER").as_deref() {
        Ok("webgpu") => vec![Provider::WebGpu],
        _ => vec![Provider::Cpu],
    };
    let mut model = Sky::load(&store, &providers).expect("the Sky model");
    let mut sam = store
        .have(&greycard_ai::SAM)
        .then_some(())
        .filter(|_| std::env::var_os("SKY_NO_SAM").is_none())
        .map(|()| Sam::load(&store, &providers).expect("SAM"));
    println!(
        "Sky model on {}, SAM {}",
        model.provider().name(),
        sam.as_ref()
            .map_or("not in the store".to_string(), |s| format!(
                "on {}",
                s.providers().0.name()
            ))
    );
    let mut stems: Vec<String> = std::fs::read_dir(&input)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .filter(|s| only.as_ref().is_none_or(|o| o.contains(s)))
        .collect();
    stems.sort();
    let mut rows = Vec::new();
    let mut painted = Vec::new();
    println!("{HEADER}");
    for stem in stems {
        let Some(linear) = read_lin(&input.join(format!("{stem}.lin"))) else {
            println!("{stem}: no linear base, passed over");
            continue;
        };
        let picture = picture(&input.join(format!("{stem}.png")));
        let (w, h) = (picture.width, picture.height);
        let t = Instant::now();
        let boxed = sky::letterbox(&picture);
        let content = (boxed.width, boxed.height);
        let logits = model.logits(boxed.planes).expect("the model");
        let t_model = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let prior = Prior::label(&logits, content, w, h);
        let t_labels = t.elapsed().as_secs_f64();
        let mut row = Row {
            stem: stem.clone(),
            labeled: prior.sky_area(),
            t_model,
            t_labels,
            size: (linear.width, linear.height),
            ..Default::default()
        };
        // The gate's core, as the gate takes it.
        let r = ((0.01 * w.max(h) as f32).round() as usize).max(1);
        let confident: Vec<bool> = prior
            .sky_kept
            .iter()
            .enumerate()
            .map(|(i, &p)| p > 0.8 && prior.is_sky(i))
            .collect();
        row.core = sky::erode(&confident, w, h, r)
            .iter()
            .filter(|&&v| v)
            .count() as f32
            / (w * h) as f32;
        let luma = picture.luma();
        let frame = Frame {
            display: &picture,
            luma: &luma,
            linear: &linear,
        };
        let mut times = sky::Times::default();
        let found = match sam.as_mut() {
            Some(s) => {
                // Embedded at the first decode, as the editor does, so
                // a frame the gate refuses pays nothing for SAM.
                let mut embedding = None;
                let mut decode = |p: &[Prompt]| {
                    if embedding.is_none() {
                        embedding = Some(s.embed(&picture)?);
                    }
                    s.decode(embedding.as_ref().unwrap(), p).map(|(m, _)| m)
                };
                let decode: &mut sky::Decode = &mut decode;
                sky::find(&prior, &frame, &[], Some(decode), (w, h), &mut times)
            }
            None => sky::find(&prior, &frame, &[], None, (w, h), &mut times),
        }
        .expect("the sky");
        row.t_outline = times.outline;
        row.t_edge = times.edge;
        let matte = match found {
            sky::Found::None(why) => {
                row.verdict = format!("none: {why}");
                Mask::new(w, h, vec![0.0; w * h])
            }
            sky::Found::Sky { matte, seeded, .. } => {
                row.verdict = "sky".to_string();
                row.found = true;
                row.seeded = seeded;
                matte
            }
        };
        let n_labels = prior
            .labels
            .iter()
            .filter(|&&l| l == sky::SKY_CLASS)
            .count();
        let (mut kept, mut spill, mut partial, mut area) = (0usize, 0usize, 0usize, 0usize);
        for (i, &a) in matte.data.iter().enumerate() {
            let l = prior.is_sky(i);
            area += (a > 0.5) as usize;
            match (a > 0.5, l) {
                (true, true) => kept += 1,
                (true, false) => spill += 1,
                _ => {}
            }
            partial += (l && a > 0.05 && a < 0.95) as usize;
        }
        let nl = n_labels.max(1) as f32;
        row.area = area as f32 / (w * h) as f32;
        row.recall = kept as f32 / nl;
        row.spill = spill as f32 / nl;
        row.partial = partial as f32 / nl;
        let inside = unlabeled_inside(&prior);
        if std::env::var_os("SKY_REGIONS").is_some() {
            for region in regions(&inside, w, h) {
                if region.len() < 20 {
                    continue;
                }
                let border = borders(&region, &inside, w, h);
                let mean = |m: &[f32], px: &[usize]| {
                    px.iter().map(|&i| m[i]).sum::<f32>() / px.len().max(1) as f32
                };
                let min = |m: &[f32], px: &[usize]| {
                    px.iter().map(|&i| m[i]).fold(f32::INFINITY, f32::min)
                };
                let (x, y) = (region[0] % w, region[0] / w);
                println!(
                    "  {stem} region at {x},{y}: {} px, sky {:.3} (min {:.3}) against border {:.3}, \
                     kept {:.3} (min {:.3}) against {:.3}, matte {:.3}",
                    region.len(),
                    mean(&prior.sky, &region),
                    min(&prior.sky, &region),
                    mean(&prior.sky, &border),
                    mean(&prior.sky_kept, &region),
                    min(&prior.sky_kept, &region),
                    mean(&prior.sky_kept, &border),
                    mean(&matte.data, &region),
                );
            }
        }
        let hole = holes(&matte);
        if row.found {
            row.inside_px = inside.iter().filter(|&&v| v).count();
            row.inside = row.inside_px as f32 / (w * h) as f32;
            row.hole_px = inside.iter().zip(&hole).filter(|(a, b)| **a && **b).count();
            row.all_holes = hole.iter().filter(|&&v| v).count() as f32 / (w * h) as f32;
        }
        if none.contains(&stem) && matte.data.iter().any(|&a| a > 0.0) {
            painted.push(stem.clone());
        }
        save_gray(&out.join(format!("{stem}.alpha.png")), w, h, matte.to_u8());
        over(&picture, &matte)
            .save(out.join(format!("{stem}.over.png")))
            .unwrap();
        if row.found {
            let marks: Vec<u8> = inside
                .iter()
                .zip(&hole)
                .map(|(&i, &o)| match (i, o) {
                    (true, true) => 255,
                    (true, false) => 110,
                    _ => 0,
                })
                .collect();
            save_gray(&out.join(format!("{stem}.holes.png")), w, h, marks);
        }
        println!("{}", row.line());
        rows.push(row);
    }
    let mut table = String::from(HEADER);
    table.push('\n');
    for r in &rows {
        table.push_str(&r.line());
        table.push('\n');
    }
    std::fs::write(out.join("table.tsv"), table).unwrap();
    let found: Vec<&Row> = rows.iter().filter(|r| r.found).collect();
    let span = |f: &dyn Fn(&Row) -> f32| {
        let v: Vec<f32> = found.iter().map(|r| f(r)).collect();
        (
            v.iter().copied().fold(f32::INFINITY, f32::min),
            v.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            v.iter().sum::<f32>() / v.len().max(1) as f32,
        )
    };
    let (rl, rh, rm) = span(&|r| r.recall);
    let (sl, sh, sm) = span(&|r| r.spill);
    println!(
        "{} frames, {} with a sky: recall {:.2}-{:.2}% (mean {:.2}), spill {:.2}-{:.2}% (mean {:.2}); \
         unlabeled inside {} px, {} of them holes; matte {:.2} s in all",
        rows.len(),
        found.len(),
        rl * 100.0,
        rh * 100.0,
        rm * 100.0,
        sl * 100.0,
        sh * 100.0,
        sm * 100.0,
        found.iter().map(|r| r.inside_px).sum::<usize>(),
        found.iter().map(|r| r.hole_px).sum::<usize>(),
        found.iter().map(|r| r.t_edge).sum::<f64>(),
    );
    assert!(painted.is_empty(), "sky painted on {painted:?}");
}
