//! The camera match's sample-size sweep: the held-out error of a
//! group's table against how many frames were developed for it and how
//! they were picked. Not shipped; an ignored test, so it measures with
//! the editor's own develop, registration, exposure match and fit
//! ([`measure`], [`fit_group`]) and nothing of its own.
//!
//! It runs in steps over a work folder, `GREYCARD_SWEEP`, each kept on
//! disk so a step is not redone and a stopped run resumes:
//!
//! 1. The survey: the raws listed in `survey-input.txt`, one path a
//!    line, grouped by body and style as the sheet groups them
//!    ([`survey_files`]); kept as `survey.tsv`.
//! 2. The sets, from `sets.tsv`: a label, the body, the style key, a
//!    lens to leave out (or nothing) and the size of the held-out
//!    pool, tab-separated. A set's candidates are the frames the
//!    sampler would take from ([`sample`]'s rule on fixed frames);
//!    the pool is drawn from them once, at random under the label,
//!    and left out of every fit, so every size and pick is measured on
//!    the same frames.
//! 3. The previews: each candidate's camera JPEG binned into cells of
//!    the table's encoded domain, and its clipped share, for the
//!    coverage pick; kept under `previews/`.
//! 4. The picks, per set and size: three random draws, the sampler as
//!    it is ([`sample`]), and the coverage pick ([`cover`]) with its
//!    cells weighted by how often they are asked about or evenly.
//! 5. The develop: every frame some pick or the pool holds, measured
//!    once as the match measures it; kept under `pairs/` with the
//!    seconds it took.
//! 6. The fits: per set, pick and size, the fitted error, the error
//!    on the pool, the sheet's held out over every frame and the
//!    earlier four-frame figure, and the seconds the match would spend
//!    (the frames' develops and the fit with its holds). Written to
//!    `results.csv`, the pool per frame to `pool.csv`, and each
//!    coverage pick's gain per frame to `coverage-<set>.csv`.
//!
//! The lens database is read from `GREYCARD_SWEEP_LENSES` (a lensfun
//! store's root) when it is set, so the run can keep its cache
//! elsewhere. Nothing is written beside a raw; the raws are only read.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use greycard_match::Pair;
use serde::{Deserialize, Serialize};

use super::cover::{Seen, cover, reach, seen_of};
use super::*;

/// The sizes swept.
const SIZES: [usize; 4] = [10, 20, 40, 80];
/// The random draws' seeds.
const SEEDS: [u64; 3] = [1, 2, 3];

/// splitmix64: a seeded stream with no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

fn seed_of(label: &str) -> u64 {
    let h = blake3::hash(label.as_bytes());
    u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap())
}

fn key_of(path: &Path) -> String {
    blake3::hash(path.as_os_str().as_encoded_bytes()).to_hex()[..16].to_string()
}

/// One set of the sweep.
struct Set {
    label: String,
    camera: String,
    style: String,
    without_lens: Option<String>,
    pool: usize,
}

fn read_sets(path: &Path) -> Vec<Set> {
    std::fs::read_to_string(path)
        .expect("sets.tsv")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Set {
                label: f[0].into(),
                camera: f[1].into(),
                style: f[2].into(),
                without_lens: f.get(3).filter(|s| !s.is_empty()).map(|s| s.to_string()),
                pool: f.get(4).and_then(|s| s.parse().ok()).unwrap_or(12),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------
// The survey.

fn survey(out: &Path) -> Vec<Group> {
    let kept = out.join("survey.tsv");
    if let Ok(text) = std::fs::read_to_string(&kept) {
        let mut groups: BTreeMap<(String, String), Group> = BTreeMap::new();
        for l in text.lines() {
            let f: Vec<&str> = l.split('\t').collect();
            let none = |s: &str| (!s.is_empty()).then(|| s.to_string());
            groups
                .entry((f[0].into(), f[1].into()))
                .or_insert_with(|| Group {
                    camera: f[0].into(),
                    make: String::new(),
                    model: String::new(),
                    maker: String::new(),
                    style: f[1].into(),
                    frames: Vec::new(),
                    look: None,
                })
                .frames
                .push(Frame {
                    fixed: f[2] == "1",
                    taken: none(f[3]),
                    lens: none(f[4]),
                    path: PathBuf::from(f[5]),
                });
        }
        let mut groups: Vec<Group> = groups.into_values().collect();
        for g in &mut groups {
            g.frames.sort_by(order);
        }
        return groups;
    }
    use rayon::prelude::*;
    let files: Vec<PathBuf> = std::fs::read_to_string(out.join("survey-input.txt"))
        .expect("survey-input.txt")
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();
    let started = Instant::now();
    let parts: Vec<Survey> = files
        .par_chunks(32)
        .map(|chunk| survey_files(chunk, &|_, _| {}, &AtomicBool::new(false)))
        .collect();
    let mut groups: BTreeMap<(String, String), Group> = BTreeMap::new();
    for s in parts {
        for g in s.groups {
            groups
                .entry((g.camera.clone(), g.style.clone()))
                .and_modify(|e| e.frames.extend(g.frames.iter().cloned()))
                .or_insert(g);
        }
    }
    let mut text = String::new();
    for g in groups.values_mut() {
        g.frames.sort_by(order);
        for f in &g.frames {
            text += &format!(
                "{}\t{}\t{}\t{}\t{}\t{}\n",
                g.camera,
                g.style,
                u8::from(f.fixed),
                f.taken.as_deref().unwrap_or(""),
                f.lens.as_deref().unwrap_or(""),
                f.path.display()
            );
        }
    }
    std::fs::write(&kept, text).unwrap();
    eprintln!(
        "survey: {} files in {:.0} s",
        files.len(),
        started.elapsed().as_secs_f32()
    );
    groups.into_values().collect()
}

/// The frames the sampler takes from: the fixed ones once there are
/// [`MIN_FRAMES`], else all.
fn candidates(g: &Group) -> Vec<Frame> {
    let fixed = g.fixed() >= MIN_FRAMES;
    g.frames
        .iter()
        .filter(|f| !fixed || f.fixed)
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------
// The previews.

fn preview_record(out: &Path, path: &Path) -> Option<Result<Seen, String>> {
    let text = std::fs::read_to_string(out.join("previews").join(key_of(path) + ".json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn previews(out: &Path, frames: &[&Frame]) -> HashMap<PathBuf, Result<Seen, String>> {
    use rayon::prelude::*;
    let dir = out.join("previews");
    std::fs::create_dir_all(&dir).unwrap();
    let started = Instant::now();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let all: Vec<(PathBuf, Result<Seen, String>)> = frames
        .par_iter()
        .map(|f| {
            if let Some(r) = preview_record(out, &f.path) {
                return (f.path.clone(), r);
            }
            let r = seen_of(&f.path);
            let file = dir.join(key_of(&f.path) + ".json");
            let part = file.with_extension("part");
            std::fs::write(&part, serde_json::to_string(&r).unwrap()).unwrap();
            std::fs::rename(&part, &file).unwrap();
            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n.is_multiple_of(50) {
                eprintln!(
                    "previews: {n} read, {:.0} s",
                    started.elapsed().as_secs_f32()
                );
            }
            (f.path.clone(), r)
        })
        .collect();
    all.into_iter().collect()
}

// ---------------------------------------------------------------------
// The develop.

#[derive(Serialize, Deserialize)]
struct Record {
    rows: usize,
    cols: usize,
    total: usize,
    /// row, col, render, camera.
    pairs: Vec<(u16, u16, [f32; 3], [f32; 3])>,
    seconds: f32,
}

fn pairs_record(out: &Path, path: &Path) -> Option<Result<Record, (String, f32)>> {
    let text = std::fs::read_to_string(out.join("pairs").join(key_of(path) + ".json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn develop_all(out: &Path, frames: &[&Frame], lenses: Option<&greycard_lens::Database>) {
    let dir = out.join("pairs");
    std::fs::create_dir_all(&dir).unwrap();
    let todo: Vec<&&Frame> = frames
        .iter()
        .filter(|f| pairs_record(out, &f.path).is_none())
        .collect();
    eprintln!(
        "develop: {} of {} frames to develop",
        todo.len(),
        frames.len()
    );
    let started = Instant::now();
    for (i, f) in todo.iter().enumerate() {
        let t = Instant::now();
        let r = measure(f, DisplayCurve::default(), lenses);
        let seconds = t.elapsed().as_secs_f32();
        let rec: Result<Record, (String, f32)> = match r {
            Ok(k) => Ok(Record {
                rows: k.pairs.rows,
                cols: k.pairs.cols,
                total: k.pairs.total,
                pairs: k
                    .pairs
                    .pairs
                    .iter()
                    .map(|p| (p.row as u16, p.col as u16, p.render, p.jpeg))
                    .collect(),
                seconds,
            }),
            Err(why) => Err((why, seconds)),
        };
        let file = dir.join(key_of(&f.path) + ".json");
        let part = file.with_extension("part");
        std::fs::write(&part, serde_json::to_string(&rec).unwrap()).unwrap();
        std::fs::rename(&part, &file).unwrap();
        if (i + 1) % 10 == 0 || i + 1 == todo.len() {
            eprintln!(
                "develop: {} of {}, {:.0} s",
                i + 1,
                todo.len(),
                started.elapsed().as_secs_f32()
            );
        }
    }
}

/// A developed frame: its pairs and the seconds its measure took, or
/// the seconds a frame the match dropped cost.
fn kept_of(out: &Path, f: &Frame) -> (Option<Kept>, f32) {
    match pairs_record(out, &f.path) {
        Some(Ok(r)) => (
            Some(Kept {
                pairs: Pairs {
                    pairs: r
                        .pairs
                        .iter()
                        .map(|&(row, col, render, jpeg)| Pair {
                            render,
                            jpeg,
                            row: row as usize,
                            col: col as usize,
                        })
                        .collect(),
                    rows: r.rows,
                    cols: r.cols,
                    total: r.total,
                },
                lens: f.lens.clone(),
            }),
            r.seconds,
        ),
        Some(Err((_, s))) => (None, s),
        None => (None, 0.0),
    }
}

// ---------------------------------------------------------------------
// The picks.

struct Pick {
    method: String,
    seed: u64,
    n: usize,
    frames: Vec<usize>,
}

/// The picks of a set from `cands`, the random ones the prefixes of
/// `orders` (one a seed, indices into `cands`).
fn picks(
    set: &Set,
    cands: &[Frame],
    orders: &[(u64, Vec<usize>)],
    seen: &[Option<&Seen>],
    out: &Path,
) -> Vec<Pick> {
    let mut all = Vec::new();
    let sizes: Vec<usize> = SIZES
        .iter()
        .map(|&n| n.min(cands.len()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for (seed, order) in orders {
        let seed = *seed;
        for &n in &sizes {
            all.push(Pick {
                method: "random".into(),
                seed,
                n,
                frames: order[..n].to_vec(),
            });
        }
    }
    let index: HashMap<&Path, usize> = cands
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.as_path(), i))
        .collect();
    for &n in &sizes {
        let frames = sample(cands, n)
            .iter()
            .map(|f| index[f.path.as_path()])
            .collect();
        all.push(Pick {
            method: "sampler".into(),
            seed: 0,
            n,
            frames,
        });
    }
    for (method, even) in [("cover", false), ("cover-even", true)] {
        for &n in &sizes {
            let c = cover(cands, seen, n, even);
            if n == *sizes.last().unwrap() {
                let mut text = String::from("rank,gain_pct,cumulative_pct,lens,forced\n");
                let mut cum = 0.0;
                for (k, step) in c.steps.iter().enumerate() {
                    cum += step.gain;
                    text += &format!(
                        "{},{:.3},{:.2},{},{}\n",
                        k + 1,
                        100.0 * step.gain,
                        100.0 * cum,
                        cands[step.frame]
                            .lens
                            .as_deref()
                            .unwrap_or("")
                            .replace(',', " "),
                        step.forced
                    );
                }
                std::fs::write(
                    out.join(format!("coverage-{}-{method}.csv", set.label)),
                    text,
                )
                .unwrap();
                eprintln!(
                    "{} {method}: {} skipped for clipping, lenses {:?}",
                    set.label, c.clipped, c.lenses
                );
            }
            all.push(Pick {
                method: method.into(),
                seed: 0,
                n,
                frames: c.steps.iter().map(|s| s.frame).collect(),
            });
        }
    }
    all
}

// ---------------------------------------------------------------------
// The fits.

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() {
        f32::NAN
    } else {
        v.iter().sum::<f32>() / v.len() as f32
    }
}

/// The sample-size sweep; see the module's doc for its inputs.
#[test]
#[ignore]
fn camera_match_sample_size_sweep() {
    let Some(out) = std::env::var_os("GREYCARD_SWEEP").map(PathBuf::from) else {
        eprintln!("set GREYCARD_SWEEP to the sweep's work folder");
        return;
    };
    let only: Option<String> = std::env::var("GREYCARD_SWEEP_SETS").ok();
    let groups = survey(&out);
    for g in &groups {
        eprintln!(
            "group {} / {}: {} frames, {} fixed",
            g.camera,
            g.style,
            g.frames.len(),
            g.fixed()
        );
    }
    let sets: Vec<Set> = read_sets(&out.join("sets.tsv"))
        .into_iter()
        .filter(|s| {
            only.as_deref()
                .is_none_or(|o| o.split(',').any(|x| x == s.label))
        })
        .collect();
    let lenses = match std::env::var_os("GREYCARD_SWEEP_LENSES") {
        Some(root) => greycard_lens::Store::at(PathBuf::from(root))
            .load()
            .map(|(db, _)| db),
        None => greycard_lens::Store::user()
            .ok()
            .and_then(|s| s.load())
            .map(|(db, _)| db),
    };
    assert!(lenses.is_some(), "no lens database");
    let develop_only = std::env::var_os("GREYCARD_SWEEP_NO_FIT").is_some();

    // Every set's candidates, pool and picks.
    struct Planned {
        cands: Vec<Frame>,
        pool: Vec<Frame>,
        picks: Vec<Pick>,
        seen: Vec<Option<Seen>>,
    }
    let mut planned = Vec::new();
    for set in &sets {
        let g = groups
            .iter()
            .find(|g| g.camera == set.camera && g.style == set.style)
            .unwrap_or_else(|| panic!("no group for {}", set.label));
        // The pool and the random orders are drawn from the group's
        // whole candidates, under the body and style, and a lens left
        // out is taken out of both after: a set without a lens holds
        // out the same frames, less that lens's, and its random picks
        // are the whole set's with that lens's frames passed over.
        let whole = candidates(g);
        let base = seed_of(&format!("{}|{}", set.camera, set.style));
        let mut order: Vec<usize> = (0..whole.len()).collect();
        Rng(base).shuffle(&mut order);
        let size = set.pool.min(whole.len());
        let in_pool: HashSet<usize> = order[..size].iter().copied().collect();
        let keep = |f: &Frame| {
            set.without_lens.as_deref().is_none_or(|w| {
                !f.lens
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&w.to_lowercase())
            })
        };
        let pool: Vec<Frame> = order[..size]
            .iter()
            .map(|&i| whole[i].clone())
            .filter(|f| keep(f))
            .collect();
        // Indices of `whole` that are candidates here, and where each
        // lands in `all`.
        let mut at: HashMap<usize, usize> = HashMap::new();
        let mut all = Vec::new();
        for (i, f) in whole.iter().enumerate() {
            if !in_pool.contains(&i) && keep(f) {
                at.insert(i, all.len());
                all.push(f.clone());
            }
        }
        let orders: Vec<(u64, Vec<usize>)> = SEEDS
            .iter()
            .map(|&seed| {
                let mut o: Vec<usize> = (0..whole.len()).collect();
                Rng(base ^ seed).shuffle(&mut o);
                (
                    seed,
                    o.into_iter().filter_map(|i| at.get(&i).copied()).collect(),
                )
            })
            .collect();
        let refs: Vec<&Frame> = all.iter().collect();
        let seen = previews(&out, &refs);
        let seen_v: Vec<Option<&Seen>> = all
            .iter()
            .map(|f| seen.get(&f.path).and_then(|r| r.as_ref().ok()))
            .collect();
        let p = picks(set, &all, &orders, &seen_v, &out);
        let seen_owned: Vec<Option<Seen>> = seen_v.iter().map(|s| s.cloned()).collect();
        eprintln!(
            "{}: {} candidates, {} in the pool",
            set.label,
            all.len(),
            pool.len()
        );
        planned.push(Planned {
            cands: all,
            pool,
            picks: p,
            seen: seen_owned,
        });
    }

    // Develop what the picks and the pools hold: the pools first, then
    // the small picks, so the first numbers come early.
    let mut todo: Vec<&Frame> = Vec::new();
    let mut have: HashSet<&Path> = HashSet::new();
    for p in &planned {
        for f in &p.pool {
            if have.insert(&f.path) {
                todo.push(f);
            }
        }
    }
    for n in SIZES {
        for p in &planned {
            for pick in p.picks.iter().filter(|k| k.n <= n) {
                for &i in &pick.frames {
                    let f = &p.cands[i];
                    if have.insert(&f.path) {
                        todo.push(f);
                    }
                }
            }
        }
    }
    if std::env::var_os("GREYCARD_SWEEP_PLAN_ONLY").is_some() {
        eprintln!("plan: {} frames to develop in all", todo.len());
        return;
    }
    develop_all(&out, &todo, lenses.as_ref());
    if develop_only {
        return;
    }

    let mut results = String::from(
        "set,method,seed,n,kept,reach,reach_even,fitted,held_pool,held_every,held_four,develop_s,fit_s,total_s\n",
    );
    let mut pool_csv = String::from("set,method,seed,n,frame,lens,delta_e\n");
    for (set, p) in sets.iter().zip(&planned) {
        let pool: Vec<(Kept, &Frame)> = p
            .pool
            .iter()
            .filter_map(|f| kept_of(&out, f).0.map(|k| (k, f)))
            .collect();
        let seen: Vec<Option<&Seen>> = p.seen.iter().map(Option::as_ref).collect();
        for pick in &p.picks {
            let mut kept = Vec::new();
            let mut develop_s = 0.0;
            for &i in &pick.frames {
                let (k, s) = kept_of(&out, &p.cands[i]);
                develop_s += s;
                kept.extend(k);
            }
            if kept.len() < 2 {
                continue;
            }
            let (x, y, ids) = stacked(&kept);
            let t = Instant::now();
            let fit = fit_group(&x, &y, &ids);
            let fit_s = t.elapsed().as_secs_f32();
            let four = evenly(kept.len(), 4);
            let held4 = mean(&leave_one_out(&x, &y, &ids, &four, LutParams::default()));
            let mut held_pool = Vec::new();
            for (k, f) in &pool {
                let (px, py, _) = stacked(std::slice::from_ref(k));
                let d = fit.model.mean_delta_e(&px, &py);
                held_pool.push(d);
                pool_csv += &format!(
                    "{},{},{},{},{},{},{:.5}\n",
                    set.label,
                    pick.method,
                    pick.seed,
                    pick.n,
                    key_of(&f.path),
                    f.lens.as_deref().unwrap_or("").replace(',', " "),
                    d
                );
            }
            let line = format!(
                "{},{},{},{},{},{:.4},{:.4},{:.5},{:.5},{:.5},{:.5},{:.1},{:.1},{:.1}\n",
                set.label,
                pick.method,
                pick.seed,
                pick.n,
                kept.len(),
                reach(&seen, &pick.frames, false),
                reach(&seen, &pick.frames, true),
                fit.fitted,
                mean(&held_pool),
                fit.held_out.unwrap_or(f32::NAN),
                held4,
                develop_s,
                fit_s,
                develop_s + fit_s
            );
            eprint!("{line}");
            results += &line;
        }
    }
    let mut f = std::fs::File::create(out.join("results.csv")).unwrap();
    f.write_all(results.as_bytes()).unwrap();
    std::fs::write(out.join("pool.csv"), pool_csv).unwrap();
}
