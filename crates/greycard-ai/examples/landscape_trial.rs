//! The landscape trial: what the Sky prior's scene labels (EoMT-S,
//! COCO panoptic, the Sky prior's notes) say of a frame beyond its sky, grouped as the
//! Landscape masks would offer them (Water, Mountain, Foliage, Grass,
//! Ground, Road, Sand, Snow, Rock, Building, and Person as an
//! exclusion), and how Sky's edge (SAM 2 seeded from the soft map, then
//! the color-line matte) does on a group other than sky. A trial, not
//! the product: the numbers it prints are for the notes.
//!
//! Set `GREYCARD_MODELS` to a store holding the Sky model (and SAM 2.1
//! for the outlines, SAM 3 for `LANDSCAPE_SAM3`), then
//!
//! ```text
//! LANDSCAPE_IN=DIR LANDSCAPE_OUT=DIR cargo run --release \
//!     -p greycard-ai --example landscape_trial
//! ```
//!
//! `LANDSCAPE_IN` holds each frame's models' preview as `<stem>.png`
//! (the editor's `ai::preview`, at most 2048 on its long side) and,
//! for the matte, its linear base as `<stem>.lin`: b"GCLN", u32 width,
//! u32 height, then f16 RGB in the working space, any size of the
//! preview's aspect (the trial box-averaged the base to 4096 on its
//! long side). The trial wrote both from an ignored greycard-ui test
//! modeled on `the_sky_over_real_frames` (develop under the default
//! edit, `ai::preview`, the base averaged down), not kept. Without a
//! `.lin` the matte is passed over and only the feathered outline is
//! made.
//!
//! - `LANDSCAPE_ONLY`: a comma list of stems.
//! - `LANDSCAPE_REFINE`: a comma list of groups to make an outline and
//!   a matte for, on each frame where the group is labeled at all.
//! - `LANDSCAPE_SAM3`: a SAM 3 phrase table (`table.py`'s format), and
//!   `LANDSCAPE_PHRASES` a comma list of its phrases to ask on each
//!   frame.
//! - `LANDSCAPE_CPU=1`: the CPU alone.
//! - `LANDSCAPE_OUTLINE_LABELS=1`: the refined groups' outline is the
//!   prior's labels, not SAM 2 seeded from the soft map;
//!   `LANDSCAPE_NO_THROUGH=1`: no class is in question near a group
//!   because the group shows through it.
//!
//! Written to `LANDSCAPE_OUT`, per frame: `<stem>.json` (every group's
//! share of the frame, the model's confidence, its queries and its
//! classes, the Sky shape's own verdict, and the times),
//! `<stem>.labels.png` (each pixel's class, 255 unlabeled),
//! `<stem>.win.png` (the winning query's class probability), one
//! `<stem>.soft.<group>.png` a group found, `<stem>.sky.png` (the Sky
//! shape's matte when its gate passes), and for each refined group
//! `.outline.<group>.png`, `.feather.<group>.png` and
//! `.matte.<group>.png` (the last at the `.lin`'s size).

use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;
use serde_json::{Value, json};

use greycard_ai::image::Mask;
use greycard_ai::sam3::{self, Presets, Sam3};
use greycard_ai::sky::{self, Logits, Prior};
use greycard_ai::{Provider, Rgb8, Sam, Sky, Store};
use greycard_core::image::WorkingImage;

/// COCO panoptic's 133 classes in the model's order (detectron2's
/// contiguous ids): the 80 things, then the 53 stuff classes.
const CLASSES: [&str; 133] = [
    "person",
    "bicycle",
    "car",
    "motorcycle",
    "airplane",
    "bus",
    "train",
    "truck",
    "boat",
    "traffic light",
    "fire hydrant",
    "stop sign",
    "parking meter",
    "bench",
    "bird",
    "cat",
    "dog",
    "horse",
    "sheep",
    "cow",
    "elephant",
    "bear",
    "zebra",
    "giraffe",
    "backpack",
    "umbrella",
    "handbag",
    "tie",
    "suitcase",
    "frisbee",
    "skis",
    "snowboard",
    "sports ball",
    "kite",
    "baseball bat",
    "baseball glove",
    "skateboard",
    "surfboard",
    "tennis racket",
    "bottle",
    "wine glass",
    "cup",
    "fork",
    "knife",
    "spoon",
    "bowl",
    "banana",
    "apple",
    "sandwich",
    "orange",
    "broccoli",
    "carrot",
    "hot dog",
    "pizza",
    "donut",
    "cake",
    "chair",
    "couch",
    "potted plant",
    "bed",
    "dining table",
    "toilet",
    "tv",
    "laptop",
    "mouse",
    "remote",
    "keyboard",
    "cell phone",
    "microwave",
    "oven",
    "toaster",
    "sink",
    "refrigerator",
    "book",
    "clock",
    "vase",
    "scissors",
    "teddy bear",
    "hair drier",
    "toothbrush",
    "banner",
    "blanket",
    "bridge",
    "cardboard",
    "counter",
    "curtain",
    "door-stuff",
    "floor-wood",
    "flower",
    "fruit",
    "gravel",
    "house",
    "light",
    "mirror-stuff",
    "net",
    "pillow",
    "platform",
    "playingfield",
    "railroad",
    "river",
    "road",
    "roof",
    "sand",
    "sea",
    "shelf",
    "snow",
    "stairs",
    "tent",
    "towel",
    "wall-brick",
    "wall-stone",
    "wall-tile",
    "wall-wood",
    "water-other",
    "window-blind",
    "window-other",
    "tree-merged",
    "fence-merged",
    "ceiling-merged",
    "sky-other-merged",
    "cabinet-merged",
    "table-merged",
    "floor-other-merged",
    "pavement-merged",
    "mountain-merged",
    "grass-merged",
    "dirt-merged",
    "paper-merged",
    "food-other-merged",
    "building-other-merged",
    "rock-merged",
    "wall-other-merged",
    "rug-merged",
];

/// A Landscape group: its name, the classes it takes, and the classes
/// it shows through (for the matte's trimap: they stay in question
/// near the group, as tree does for sky).
struct Group {
    name: &'static str,
    classes: &'static [u8],
    through: &'static [u8],
}

const GROUPS: &[Group] = &[
    Group {
        name: "sky",
        classes: &[119],
        through: &[116, 88],
    },
    Group {
        name: "water",
        classes: &[99, 103, 113],
        through: &[],
    },
    Group {
        name: "mountain",
        classes: &[124],
        through: &[116],
    },
    Group {
        name: "foliage",
        classes: &[116],
        through: &[119],
    },
    Group {
        name: "flower",
        classes: &[88],
        through: &[],
    },
    Group {
        name: "grass",
        classes: &[125],
        through: &[],
    },
    Group {
        name: "ground",
        classes: &[126, 90, 97],
        through: &[],
    },
    Group {
        name: "road",
        classes: &[100, 123, 98],
        through: &[],
    },
    Group {
        name: "sand",
        classes: &[102],
        through: &[],
    },
    Group {
        name: "snow",
        classes: &[105],
        through: &[],
    },
    Group {
        name: "rock",
        classes: &[130],
        through: &[],
    },
    Group {
        name: "building",
        classes: &[129, 91, 101],
        through: &[116],
    },
    Group {
        name: "wall",
        classes: &[109, 110, 111, 112, 131],
        through: &[116],
    },
    Group {
        name: "bridge",
        classes: &[82],
        through: &[],
    },
    Group {
        name: "person",
        classes: &[0],
        through: &[],
    },
];

const KEEP: f32 = 0.5;
const WORK: usize = 1024;

fn group_of(class: u8) -> Option<usize> {
    GROUPS.iter().position(|g| g.classes.contains(&class))
}

struct Query {
    p: Vec<f32>,
    best: f32,
    label: u8,
    kept: bool,
    any: f32,
}

fn queries(class: &[f32]) -> Vec<Query> {
    class
        .chunks(sky::CLASSES)
        .map(|logits| {
            let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let exp: Vec<f32> = logits.iter().map(|l| (l - max).exp()).collect();
            let total: f32 = exp.iter().sum();
            let p: Vec<f32> = exp[..sky::CLASSES - 1].iter().map(|e| e / total).collect();
            let (label, best) =
                p.iter().enumerate().fold(
                    (0, f32::NEG_INFINITY),
                    |a, (i, &v)| if v > a.1 { (i, v) } else { a },
                );
            Query {
                any: p.iter().sum(),
                p,
                best,
                label: label as u8,
                kept: best >= KEEP,
            }
        })
        .collect()
}

/// The labeling rule of `sky::Prior::label` without its sky doubt, at
/// `w` × `h`: each pixel's class (255 unlabeled), its winning query
/// (or `u8::MAX as u16`), that query's class probability, and each
/// group's soft map: Σ p(group|q)·m_q over the kept queries whose class
/// is in the group, over Σ p(any|q)·m_q over the kept queries (over
/// all of them, Sky's rule, with `LANDSCAPE_ALL_QUERIES`).
struct Labels {
    w: usize,
    h: usize,
    labels: Vec<u8>,
    winner: Vec<u16>,
    win_p: Vec<f32>,
    soft: Vec<Vec<f32>>,
}

/// One row of [`Labels`]: classes, winners, their probabilities, and
/// each group's soft map.
type Row = (Vec<u8>, Vec<u16>, Vec<f32>, Vec<Vec<f32>>);

fn label(logits: &Logits, qs: &[Query], content: (usize, usize), w: usize, h: usize) -> Labels {
    let side = sky::MASK_SIDE as f32 / sky::SIZE as f32;
    let ms = sky::MASK_SIDE;
    let mw = ((content.0 as f32 * side).round() as usize).clamp(1, ms);
    let mh = ((content.1 as f32 * side).round() as usize).clamp(1, ms);
    let axis = |from: usize, to: usize| -> Vec<(usize, usize, f32)> {
        let s = from as f32 / to as f32;
        (0..to)
            .map(|o| {
                let f = ((o as f32 + 0.5) * s - 0.5).max(0.0);
                let a = (f as usize).min(from - 1);
                (a, (a + 1).min(from - 1), f - a as f32)
            })
            .collect()
    };
    let xs = axis(mw, w);
    let ys = axis(mh, h);
    let groups: Vec<Option<usize>> = qs
        .iter()
        .map(|q| if q.kept { group_of(q.label) } else { None })
        .collect();
    let rows: Vec<Row> = (0..h)
        .into_par_iter()
        .map(|y| {
            let (y0, y1, ty) = ys[y];
            let mut den = vec![0.0f32; w];
            let mut den_kept = vec![0.0f32; w];
            let mut num = vec![vec![0.0f32; w]; GROUPS.len()];
            let mut best = vec![f32::NEG_INFINITY; w];
            let mut win_m = vec![0.0f32; w];
            let mut win_q = vec![u16::MAX; w];
            for (q, query) in qs.iter().enumerate() {
                let plane = &logits.masks[q * ms * ms..(q + 1) * ms * ms];
                let (r0, r1) = (&plane[y0 * ms..y0 * ms + ms], &plane[y1 * ms..y1 * ms + ms]);
                for (x, &(x0, x1, tx)) in xs.iter().enumerate() {
                    let top = r0[x0] + (r0[x1] - r0[x0]) * tx;
                    let bottom = r1[x0] + (r1[x1] - r1[x0]) * tx;
                    let m = 1.0 / (1.0 + (-(top + (bottom - top) * ty)).exp());
                    den[x] += query.any * m;
                    if query.kept {
                        den_kept[x] += query.any * m;
                    }
                    if let Some(g) = groups[q] {
                        let pg: f32 = GROUPS[g].classes.iter().map(|&c| query.p[c as usize]).sum();
                        num[g][x] += pg * m;
                    }
                    if query.kept {
                        let p = query.best * m;
                        if p > best[x] {
                            best[x] = p;
                            win_m[x] = m;
                            win_q[x] = q as u16;
                        }
                    }
                }
            }
            let mut labels = vec![255u8; w];
            let mut winner = vec![u16::MAX; w];
            let mut win_p = vec![0.0f32; w];
            for x in 0..w {
                if win_m[x] >= 0.5 {
                    let q = win_q[x] as usize;
                    labels[x] = qs[q].label;
                    winner[x] = q as u16;
                    win_p[x] = qs[q].best;
                }
            }
            // Over every query (Sky's rule), or over the kept ones alone:
            // the leftover low-probability queries dilute a group the
            // model is less sure of than it is of a sky (the Sky gate's 0.9 core).
            let over = if std::env::var_os("LANDSCAPE_ALL_QUERIES").is_some() {
                &den
            } else {
                &den_kept
            };
            let soft = num
                .into_iter()
                .map(|n| {
                    n.iter()
                        .zip(over)
                        .map(|(a, d)| (a / d.max(1e-6)).clamp(0.0, 1.0))
                        .collect()
                })
                .collect();
            (labels, winner, win_p, soft)
        })
        .collect();
    let mut out = Labels {
        w,
        h,
        labels: Vec::with_capacity(w * h),
        winner: Vec::with_capacity(w * h),
        win_p: Vec::with_capacity(w * h),
        soft: vec![Vec::with_capacity(w * h); GROUPS.len()],
    };
    for (l, q, p, s) in rows {
        out.labels.extend(l);
        out.winner.extend(q);
        out.win_p.extend(p);
        for (o, s) in out.soft.iter_mut().zip(s) {
            o.extend(s);
        }
    }
    out
}

/// `labels` brought to `w` × `h`: classes, winners and probabilities to
/// the nearest pixel, the soft maps bilinearly.
fn brought(l: Labels, w: usize, h: usize) -> Labels {
    if (l.w, l.h) == (w, h) {
        return l;
    }
    let idx: Vec<usize> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            ((y * l.h) / h).min(l.h - 1) * l.w + ((x * l.w) / w).min(l.w - 1)
        })
        .collect();
    Labels {
        w,
        h,
        labels: idx.iter().map(|&i| l.labels[i]).collect(),
        winner: idx.iter().map(|&i| l.winner[i]).collect(),
        win_p: idx.iter().map(|&i| l.win_p[i]).collect(),
        soft: l
            .soft
            .into_iter()
            .map(|s| Mask::new(l.w, l.h, s).resampled(w, h).data)
            .collect(),
    }
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

fn save_gray(path: &Path, w: usize, h: usize, data: Vec<u8>) {
    image::GrayImage::from_raw(w as u32, h as u32, data)
        .expect("a plane of the size")
        .save(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn save_mask(path: &Path, m: &Mask) {
    save_gray(path, m.width, m.height, m.to_u8());
}

/// The prior for `group` as Sky's outline and matte read one: the
/// group's soft map as the "sky" map, its classes labeled sky, the
/// classes it shows through labeled tree (in question near it), the
/// real sky, tree and flower moved to stuff ids nothing treats
/// specially, and the things left as they are (known not-group).
fn as_sky(l: &Labels, g: usize, best: f32) -> Prior {
    let group = &GROUPS[g];
    let labels = l
        .labels
        .iter()
        .map(|&c| {
            if group.classes.contains(&c) {
                sky::SKY_CLASS
            } else if group.through.contains(&c)
                && std::env::var_os("LANDSCAPE_NO_THROUGH").is_none()
            {
                greycard_ai::matte::TREE_CLASS
            } else if c == sky::SKY_CLASS {
                202
            } else if c == greycard_ai::matte::TREE_CLASS {
                200
            } else if c == greycard_ai::matte::FLOWER_CLASS {
                201
            } else {
                c
            }
        })
        .collect();
    Prior {
        width: l.w,
        height: l.h,
        sky: l.soft[g].clone(),
        sky_kept: l.soft[g].clone(),
        labels,
        sure: best,
    }
}

fn main() {
    let models = std::env::var_os("GREYCARD_MODELS").expect("GREYCARD_MODELS names the store");
    let input = PathBuf::from(std::env::var_os("LANDSCAPE_IN").expect("LANDSCAPE_IN"));
    let out = PathBuf::from(std::env::var_os("LANDSCAPE_OUT").expect("LANDSCAPE_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let list = |name: &str| -> Option<Vec<String>> {
        std::env::var(name).ok().map(|s| {
            s.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
    };
    let only = list("LANDSCAPE_ONLY");
    let refine: Vec<String> = list("LANDSCAPE_REFINE").unwrap_or_default();
    let phrases: Vec<String> = list("LANDSCAPE_PHRASES").unwrap_or_default();
    let providers = if std::env::var_os("LANDSCAPE_CPU").is_some() {
        vec![Provider::Cpu]
    } else {
        Provider::available()
    };
    let store = Store::at(models);

    let t = Instant::now();
    let mut model = Sky::load(&store, &providers).expect("the Sky model");
    println!(
        "Sky model on {} in {:.2}s",
        model.provider().name(),
        t.elapsed().as_secs_f64()
    );
    let mut sam: Option<Sam> = None;
    let mut sam3: Option<(Sam3, Presets)> = std::env::var_os("LANDSCAPE_SAM3").map(|table| {
        let t = Instant::now();
        let presets = Presets::read(Path::new(&table)).expect("the phrase table");
        let s = Sam3::load(&store, &providers).expect("SAM 3");
        println!(
            "SAM 3 on {:?} in {:.2}s",
            s.providers(),
            t.elapsed().as_secs_f64()
        );
        (s, presets)
    });

    let mut stems: Vec<String> = std::fs::read_dir(&input)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".png").map(str::to_string)
        })
        .collect();
    stems.sort();
    for stem in stems {
        if only.as_ref().is_some_and(|o| !o.contains(&stem)) {
            continue;
        }
        let picture = image::open(input.join(format!("{stem}.png")))
            .unwrap_or_else(|e| panic!("{stem}: {e}"))
            .to_rgb8();
        let (w, h) = (picture.width() as usize, picture.height() as usize);
        let rgb = Rgb8::new(w, h, picture.into_raw());
        let linear = read_lin(&input.join(format!("{stem}.lin")));
        let mut times = serde_json::Map::new();

        // The model, once.
        let t = Instant::now();
        let boxed = sky::letterbox(&rgb);
        let content = (boxed.width, boxed.height);
        let t_model = Instant::now();
        let logits = model.logits(boxed.planes).expect("the model");
        times.insert("model".into(), json!(t_model.elapsed().as_secs_f64()));
        times.insert(
            "letterbox_and_model".into(),
            json!(t.elapsed().as_secs_f64()),
        );

        // The Sky shape's own verdict and matte, as the product makes it.
        let t = Instant::now();
        let prior = Prior::label(&logits, content, w, h);
        times.insert("sky_prior_labels".into(), json!(t.elapsed().as_secs_f64()));
        let gate = sky::gate(&prior);
        let mut sky_json = json!({
            "sure": prior.sure,
            "labeled": prior.sky_area(),
            "gate": match &gate { Ok(()) => "passes".to_string(), Err(why) => why.to_string() },
        });
        let mut embedding = None;
        let mut sam_time = 0.0;
        let mut ensure_embedding =
            |sam: &mut Option<Sam>, embedding: &mut Option<greycard_ai::sam::Embedding>| {
                if sam.is_none() && store.have(&greycard_ai::SAM) {
                    *sam = Some(Sam::load(&store, &providers).expect("SAM 2"));
                }
                if embedding.is_none()
                    && let Some(s) = sam.as_mut()
                {
                    let t = Instant::now();
                    *embedding = Some(s.embed(&rgb).expect("SAM 2 embed"));
                    sam_time = t.elapsed().as_secs_f64();
                }
            };
        if gate.is_ok() {
            let t = Instant::now();
            ensure_embedding(&mut sam, &mut embedding);
            let luma = rgb.luma();
            let fallback = WorkingImage::new(0, 0);
            let frame = sky::Frame {
                display: &rgb,
                luma: &luma,
                linear: linear.as_ref().unwrap_or(&fallback),
            };
            let mut st = sky::Times::default();
            let found = {
                let s = &mut sam;
                let e = &embedding;
                let mut decode =
                    |prompts: &[greycard_ai::Prompt]| -> greycard_ai::runtime::Result<Mask> {
                        s.as_mut()
                            .unwrap()
                            .decode(e.as_ref().unwrap(), prompts)
                            .map(|(m, _)| m)
                    };
                let decode: Option<&mut sky::Decode> =
                    if e.is_some() { Some(&mut decode) } else { None };
                sky::find(&prior, &frame, &[], decode, (w, h), &mut st).expect("sky::find")
            };
            if let sky::Found::Sky { matte, .. } = found {
                let area = matte.data.iter().filter(|&&v| v > 0.5).count() as f32 / (w * h) as f32;
                sky_json["matte_area"] = json!(area);
                save_mask(&out.join(format!("{stem}.sky.png")), &matte);
            }
            times.insert("sky_find".into(), json!(t.elapsed().as_secs_f64()));
            times.insert("sky_edge".into(), json!(st.edge));
        }

        // Every group from the same answer.
        let t = Instant::now();
        let qs = queries(&logits.class);
        let long = w.max(h);
        let s = (WORK as f64 / long as f64).min(1.0);
        let (ww, wh) = (
            ((w as f64 * s).round() as usize).max(1),
            ((h as f64 * s).round() as usize).max(1),
        );
        let l = brought(label(&logits, &qs, content, ww, wh), w, h);
        times.insert("group_labels".into(), json!(t.elapsed().as_secs_f64()));
        save_gray(
            &out.join(format!("{stem}.labels.png")),
            w,
            h,
            l.labels.clone(),
        );
        save_gray(
            &out.join(format!("{stem}.win.png")),
            w,
            h,
            l.win_p.iter().map(|p| (p * 255.0).round() as u8).collect(),
        );
        let n = (w * h) as f32;
        let erosion = ((0.01 * long as f32).round() as usize).max(1);
        let mut groups = Vec::new();
        let mut bests = vec![0.0f32; GROUPS.len()];
        for (g, group) in GROUPS.iter().enumerate() {
            let pix: Vec<usize> = (0..w * h)
                .filter(|&i| group.classes.contains(&l.labels[i]))
                .collect();
            if pix.is_empty() {
                continue;
            }
            let area = pix.len() as f32 / n;
            let mut per_query: std::collections::BTreeMap<u16, usize> = Default::default();
            let mut per_class: std::collections::BTreeMap<u8, usize> = Default::default();
            let (mut psum, mut sure90, mut sure98) = (0.0f64, 0usize, 0usize);
            for &i in &pix {
                *per_query.entry(l.winner[i]).or_default() += 1;
                *per_class.entry(l.labels[i]).or_default() += 1;
                psum += l.win_p[i] as f64;
                sure90 += (l.win_p[i] >= 0.9) as usize;
                sure98 += (l.win_p[i] >= 0.98) as usize;
            }
            let mut qlist: Vec<Value> = per_query
                .iter()
                .map(|(&q, &c)| {
                    let q = &qs[q as usize];
                    json!({"class": CLASSES[q.label as usize], "p": q.best, "area": c as f32 / n})
                })
                .collect();
            qlist.sort_by(|a, b| b["area"].as_f64().partial_cmp(&a["area"].as_f64()).unwrap());
            let best = per_query
                .keys()
                .map(|&q| qs[q as usize].best)
                .fold(0.0f32, f32::max);
            bests[g] = best;
            let confident: Vec<bool> = l.soft[g].iter().map(|&p| p > 0.8).collect();
            let core = sky::erode(&confident, w, h, erosion)
                .iter()
                .filter(|&&v| v)
                .count() as f32
                / n;
            // Where it reaches: which edges of the frame it touches.
            let r = erosion;
            let touches = |f: &dyn Fn(usize, usize) -> bool| pix.iter().any(|&i| f(i % w, i / w));
            let edges = json!({
                "top": touches(&|_, y| y < r),
                "bottom": touches(&|_, y| y + r >= h),
                "left": touches(&|x, _| x < r),
                "right": touches(&|x, _| x + r >= w),
            });
            // Where it sits: the share of its pixels in the frame's top half.
            let upper = pix.iter().filter(|&&i| i / w < h / 2).count() as f32 / pix.len() as f32;
            groups.push(json!({
                "group": group.name,
                "area": area,
                "best": best,
                "mean_p": psum / pix.len() as f64,
                "sure90": sure90 as f32 / n,
                "sure98": sure98 as f32 / n,
                "core": core,
                "upper": upper,
                "edges": edges,
                "queries": qlist,
                "classes": per_class.iter().map(|(&c, &k)| (CLASSES[c as usize].to_string(), json!(k as f32 / n))).collect::<serde_json::Map<_, _>>(),
            }));
            save_gray(
                &out.join(format!("{stem}.soft.{}.png", group.name)),
                w,
                h,
                l.soft[g]
                    .iter()
                    .map(|p| (p * 255.0).round() as u8)
                    .collect(),
            );
        }
        // Every kept query, for reading by eye.
        let kept: Vec<Value> = qs
            .iter()
            .filter(|q| q.kept)
            .map(|q| json!({"class": CLASSES[q.label as usize], "p": q.best}))
            .collect();

        // The edge, on the groups asked for.
        let mut refined = serde_json::Map::new();
        for name in &refine {
            let Some(g) = GROUPS.iter().position(|g| g.name == name) else {
                continue;
            };
            if bests[g] == 0.0 {
                continue;
            }
            let prior = as_sky(&l, g, bests[g]);
            let t = Instant::now();
            ensure_embedding(&mut sam, &mut embedding);
            let seeds = sky::seeds(&prior);
            let by_labels = std::env::var_os("LANDSCAPE_OUTLINE_LABELS").is_some();
            let outline = match (sam.as_mut().filter(|_| !by_labels), embedding.as_ref()) {
                (Some(s), Some(e)) => {
                    sky::outline(&prior, &seeds, &[], |p| s.decode(e, p).map(|(m, _)| m))
                        .expect("outline")
                }
                _ => prior.labeled_sky(),
            };
            let t_outline = t.elapsed().as_secs_f64();
            save_mask(&out.join(format!("{stem}.outline.{name}.png")), &outline);
            let luma = rgb.luma();
            let fallback = WorkingImage::new(0, 0);
            let frame = sky::Frame {
                display: &rgb,
                luma: &luma,
                linear: linear.as_ref().unwrap_or(&fallback),
            };
            let t = Instant::now();
            let feather = sky::feathered(&outline, &frame);
            let t_feather = t.elapsed().as_secs_f64();
            save_mask(&out.join(format!("{stem}.feather.{name}.png")), &feather);
            let t = Instant::now();
            let matte = linear.as_ref().and_then(|lin| {
                greycard_ai::matte::color_line(&outline, &prior, lin, &[], lin.width, lin.height)
            });
            let t_matte = t.elapsed().as_secs_f64();
            if let Some(m) = &matte {
                save_mask(&out.join(format!("{stem}.matte.{name}.png")), m);
            }
            refined.insert(
                name.clone(),
                json!({
                    "seeds": [seeds.positive.len(), seeds.negative.len()],
                    "outline_area": outline.data.iter().filter(|&&v| v > 0.5).count() as f32 / n,
                    "outline_s": t_outline,
                    "feather_s": t_feather,
                    "matte_s": t_matte,
                    "matte": matte.is_some(),
                }),
            );
        }
        if sam_time > 0.0 {
            times.insert("sam2_embed".into(), json!(sam_time));
        }

        // SAM 3 by phrase.
        let mut by_phrase = serde_json::Map::new();
        if let Some((s3, presets)) = sam3.as_mut()
            && !phrases.is_empty()
        {
            let t = Instant::now();
            let enc = s3.encode(&rgb).expect("SAM 3 encode");
            times.insert("sam3_encode".into(), json!(t.elapsed().as_secs_f64()));
            for phrase in &phrases {
                let Some(p) = presets.phrase(phrase) else {
                    println!("{phrase}: not in the table");
                    continue;
                };
                let t = Instant::now();
                let found = s3.decode(&enc, p).expect("SAM 3 decode");
                let dt = t.elapsed().as_secs_f64();
                let over: Vec<_> = found.over(sam3::CUT).cloned().collect();
                let scores: Vec<f32> = found.instances.iter().take(5).map(|i| i.score).collect();
                let mut area = 0.0;
                if !over.is_empty() {
                    let m = sam3::union(&over, sam3::CUT).resampled(w, h);
                    area = m.data.iter().filter(|&&v| v > 0.5).count() as f32 / n;
                    save_mask(
                        &out.join(format!("{stem}.sam3.{}.png", phrase.replace(' ', "_"))),
                        &m,
                    );
                }
                by_phrase.insert(
                    phrase.clone(),
                    json!({"presence": found.presence, "scores": scores, "over": over.len(), "area": area, "decode_s": dt}),
                );
            }
        }

        let report = json!({
            "stem": stem,
            "size": [w, h],
            "linear": linear.as_ref().map(|l| [l.width, l.height]),
            "sky": sky_json,
            "groups": groups,
            "kept": kept,
            "refined": refined,
            "sam3": by_phrase,
            "times": times,
        });
        std::fs::write(
            out.join(format!("{stem}.json")),
            serde_json::to_string_pretty(&report).unwrap(),
        )
        .unwrap();
        let found: Vec<String> = report["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                format!(
                    "{} {:.1}% ({:.3})",
                    g["group"].as_str().unwrap(),
                    g["area"].as_f64().unwrap() * 100.0,
                    g["best"].as_f64().unwrap()
                )
            })
            .collect();
        println!(
            "{stem}: model {:.3}s | sky {} | {}",
            report["times"]["model"].as_f64().unwrap(),
            report["sky"]["gate"].as_str().unwrap(),
            found.join(", ")
        );
    }
}
